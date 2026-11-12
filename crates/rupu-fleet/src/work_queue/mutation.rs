use super::{
    disk::{self, Store},
    types::*,
    validation::*,
};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use sha2::{Digest, Sha256};

#[cfg(test)]
thread_local! { pub(crate) static FAULT: std::cell::Cell<u8> = const {std::cell::Cell::new(0)}; }

fn keys(command: &WorkCommand, context: &WorkContext) -> Result<(String, String), WorkError> {
    let actor = disk::encode(&context.actor)?;
    let payload = disk::encode(command)?;
    Ok((
        format!("{:x}", Sha256::digest(actor.as_bytes())),
        format!("{:x}", Sha256::digest(payload.as_bytes())),
    ))
}

fn replay(
    conn: &Connection,
    command: &WorkCommand,
    actor: &str,
    digest: &str,
) -> Result<Option<WorkReply>, WorkError> {
    let found: Option<(String, String)> = conn
        .query_row(
            "SELECT digest,payload FROM receipts WHERE actor=?1 AND request_id=?2",
            params![actor, command.request_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(disk::sql_error)?;
    let Some((saved_digest, raw)) = found else {
        return Ok(None);
    };
    if saved_digest != digest {
        return Err(WorkError::IdempotencyConflict {
            request_id: command.request_id.clone(),
        });
    }
    let receipt: WorkReceipt = disk::decode(&raw)?;
    let meta = disk::read_meta(conn)?;
    if receipt.request_id != command.request_id
        || Some(&receipt.queue_id) != meta.queue_id.as_ref()
        || receipt.queue_revision > meta.revision
    {
        return Err(WorkError::Corrupt("Queue receipt identity mismatch".into()));
    }
    let current_item = disk::item(conn, &receipt.work_id)?;
    Ok(Some(WorkReply {
        receipt,
        replayed: true,
        current_item,
        queue_revision: meta.revision,
    }))
}

impl Store {
    pub fn replay(
        &self,
        command: &WorkCommand,
        context: &WorkContext,
    ) -> Result<Option<WorkReply>, WorkError> {
        let (actor, digest) = keys(command, context)?;
        let tx = self.conn.unchecked_transaction().map_err(disk::sql_error)?;
        let result = replay(&tx, command, &actor, &digest)?;
        self.check_location()?;
        tx.commit().map_err(disk::sql_error)?;
        Ok(result)
    }

    pub fn apply(
        &mut self,
        command: &WorkCommand,
        context: &WorkContext,
    ) -> Result<WorkReply, WorkError> {
        validate_context(context)?;
        validate_command(command, context)?;
        self.check_location()?;
        let (actor_key, digest) = keys(command, context)?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(disk::sql_error)?;
        if let Some(reply) = replay(&tx, command, &actor_key, &digest)? {
            tx.commit().map_err(disk::sql_error)?;
            return Ok(reply);
        }
        let mut meta = disk::read_meta(&tx)?;
        if command.expected_queue_id != meta.queue_id {
            return Err(WorkError::OwnerChanged);
        }
        let old = match command.operation.work_id() {
            Some(id) => Some(disk::item(&tx, id)?),
            None => None,
        };
        let revision = old.as_ref().map_or(meta.revision, |i| i.revision);
        if command.expected_revision != revision {
            return Err(WorkError::RevisionConflict {
                expected: command.expected_revision,
                current: revision,
            });
        }
        let queue_revision = checked_increment(meta.revision)?;
        let item_revision = if old.is_some() {
            checked_increment(revision)?
        } else {
            1
        };
        let at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let event_id = format!("work_event_{}", ulid::Ulid::new());
        let mut item = if let Some(previous) = &old {
            previous.clone()
        } else {
            let WorkOperation::Post {
                title,
                instructions,
            } = &command.operation
            else {
                return Err(WorkError::InvalidInput(
                    "item action requires existing item".into(),
                ));
            };
            let work_id = format!("work_{}", ulid::Ulid::new());
            WorkItem {
                work_id: work_id.clone(),
                creation_sequence: checked_increment(meta.items)?,
                revision: item_revision,
                title: title.clone(),
                instructions: instructions.clone(),
                creator: context.actor.clone(),
                posted_at: at.clone(),
                updated_at: at.clone(),
                state: WorkState::Queued,
                latest_attempt: None,
                latest_event: WorkEvent {
                    event_id: event_id.clone(),
                    sequence: queue_revision,
                    work_id,
                    item_revision,
                    queue_revision,
                    work_attempt_id: None,
                    action: WorkAction::Post,
                    at: at.clone(),
                    actor: context.actor.clone(),
                    source: context.source.clone(),
                    from_state: None,
                    to_state: WorkState::Queued,
                    summary: None,
                    prior_attempt_may_still_run: false,
                },
                prior_attempt_may_still_run: false,
                attempt_count: 0,
                event_count: 0,
            }
        };
        let prior_risk = transition(
            &mut item,
            &command.operation,
            &context.actor,
            &at,
            &event_id,
        )?;
        item.revision = item_revision;
        item.event_count = checked_increment(item.event_count)?;
        item.updated_at = at.clone();
        item.prior_attempt_may_still_run |= prior_risk;
        let attempt_id = item
            .latest_attempt
            .as_ref()
            .map(|a| a.work_attempt_id.clone());
        let event = WorkEvent {
            event_id: event_id.clone(),
            sequence: queue_revision,
            work_id: item.work_id.clone(),
            item_revision,
            queue_revision,
            work_attempt_id: attempt_id.clone(),
            action: command.operation.action(),
            at: at.clone(),
            actor: context.actor.clone(),
            source: context.source.clone(),
            from_state: old.as_ref().map(|i| i.state),
            to_state: item.state,
            summary: command.operation.summary().map(str::to_owned),
            prior_attempt_may_still_run: prior_risk,
        };
        item.latest_event = event.clone();
        let queue_id = meta
            .queue_id
            .clone()
            .unwrap_or_else(|| format!("work_queue_{}", ulid::Ulid::new()));
        let receipt = WorkReceipt {
            request_id: command.request_id.clone(),
            queue_id: queue_id.clone(),
            work_id: item.work_id.clone(),
            item_revision,
            queue_revision,
            event_id: event_id.clone(),
            work_attempt_id: attempt_id,
            committed_at: at,
            execution_effect: WorkExecutionEffect::None,
            prior_attempt_may_still_run: prior_risk,
        };
        let payload = disk::encode(&item)?;
        let summary = disk::encode(&summary(&item))?;
        tx.execute("INSERT INTO items(id,creation_sequence,revision,state,claimant,payload,summary) VALUES(?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(id) DO UPDATE SET revision=excluded.revision,state=excluded.state,claimant=excluded.claimant,payload=excluded.payload,summary=excluded.summary",params![item.work_id,item.creation_sequence as i64,item.revision as i64,item.state.key(),claimant(&item).unwrap_or(""),payload,summary]).map_err(disk::sql_error)?;
        if let Some(attempt) = &item.latest_attempt {
            tx.execute("INSERT INTO attempts(id,item_id,number,payload) VALUES(?1,?2,?3,?4) ON CONFLICT(id) DO UPDATE SET payload=excluded.payload",params![attempt.work_attempt_id,item.work_id,attempt.number as i64,disk::encode(attempt)?]).map_err(disk::sql_error)?;
        }
        tx.execute(
            "INSERT INTO events(sequence,id,item_id,item_revision,payload) VALUES(?1,?2,?3,?4,?5)",
            params![
                queue_revision as i64,
                event_id,
                item.work_id,
                item.revision as i64,
                disk::encode(&event)?
            ],
        )
        .map_err(disk::sql_error)?;
        tx.execute(
            "INSERT INTO receipts(actor,request_id,digest,payload) VALUES(?1,?2,?3,?4)",
            params![
                actor_key,
                command.request_id,
                digest,
                disk::encode(&receipt)?
            ],
        )
        .map_err(disk::sql_error)?;
        if let Some(previous) = &old {
            count(&tx, previous, -1)?;
        }
        count(&tx, &item, 1)?;
        meta.queue_id = Some(queue_id);
        meta.revision = queue_revision;
        meta.events = checked_increment(meta.events)?;
        meta.receipts = checked_increment(meta.receipts)?;
        if old.is_none() {
            meta.items = checked_increment(meta.items)?;
        }
        let was_open = old.as_ref().is_some_and(|i| !i.state.is_terminal());
        let is_open = !item.state.is_terminal();
        if !was_open && is_open {
            meta.open_items = checked_increment(meta.open_items)?;
        }
        if was_open && !is_open {
            meta.open_items = meta
                .open_items
                .checked_sub(1)
                .ok_or_else(|| WorkError::Corrupt("invalid unresolved-work count".into()))?;
        }
        tx.execute("UPDATE meta SET queue_id=?1,revision=?2,item_count=?3,event_count=?4,receipt_count=?5,open_items=?6 WHERE singleton=1",params![meta.queue_id,meta.revision as i64,meta.items as i64,meta.events as i64,meta.receipts as i64,meta.open_items as i64]).map_err(disk::sql_error)?;
        let snapshot = disk::snapshot(&tx, &meta)?;
        if is_open
            && snapshot
                .capacity
                .database_bytes
                .saturating_add(snapshot.capacity.reserved_bytes)
                > meta.max_bytes
        {
            return Err(WorkError::Capacity {
                resource: WorkCapacityResource::DatabaseBytes,
                limit: meta.max_bytes,
                current: snapshot.capacity.database_bytes,
                requested: snapshot.capacity.reserved_bytes,
            });
        }
        #[cfg(test)]
        if FAULT.with(|f| f.get()) == 1 {
            return Err(WorkError::Persistence("injected before commit".into()));
        }
        tx.commit().map_err(|e| WorkError::Reconciliation {
            request_id: command.request_id.clone(),
            message: e.to_string(),
        })?;
        #[cfg(test)]
        if FAULT.with(|f| f.get()) == 2 {
            return Err(WorkError::Reconciliation {
                request_id: command.request_id.clone(),
                message: "injected after commit".into(),
            });
        }
        self.check_location()
            .map_err(|e| WorkError::Reconciliation {
                request_id: command.request_id.clone(),
                message: e.to_string(),
            })?;
        Ok(WorkReply {
            receipt,
            replayed: false,
            current_item: item,
            queue_revision,
        })
    }
}

fn count(conn: &Connection, item: &WorkItem, delta: i64) -> Result<(), WorkError> {
    for (state, actor) in [
        ("*", "*"),
        (item.state.key(), "*"),
        ("*", claimant(item).unwrap_or("")),
        (item.state.key(), claimant(item).unwrap_or("")),
    ] {
        if delta > 0 {
            conn.execute("INSERT INTO counts(state,claimant,count) VALUES(?1,?2,1) ON CONFLICT(state,claimant) DO UPDATE SET count=count+1",params![state,actor]).map_err(disk::sql_error)?;
        } else if conn
            .execute(
                "UPDATE counts SET count=count-1 WHERE state=?1 AND claimant=?2 AND count>0",
                params![state, actor],
            )
            .map_err(disk::sql_error)?
            != 1
        {
            return Err(WorkError::Corrupt(
                "Queue count index disagrees with item".into(),
            ));
        }
    }
    Ok(())
}

fn transition(
    item: &mut WorkItem,
    op: &WorkOperation,
    actor: &WorkActor,
    at: &str,
    event_id: &str,
) -> Result<bool, WorkError> {
    let allowed = match op {
        WorkOperation::Post { .. } => true,
        WorkOperation::Claim { .. } => item.state == WorkState::Queued,
        WorkOperation::Start { .. } => item.state == WorkState::Claimed,
        WorkOperation::Block { .. } => item.state == WorkState::Working,
        WorkOperation::Resume { .. } => item.state == WorkState::Blocked,
        WorkOperation::Complete { .. } => {
            matches!(item.state, WorkState::Working | WorkState::Blocked)
        }
        WorkOperation::Fail { .. } => matches!(
            item.state,
            WorkState::Claimed | WorkState::Working | WorkState::Blocked
        ),
        WorkOperation::Cancel { .. } => !item.state.is_terminal(),
        WorkOperation::Requeue { .. } => item.state != WorkState::Queued,
    };
    if !allowed {
        return Err(WorkError::TransitionConflict {
            state: item.state,
            action: op.action(),
        });
    }
    if let Some(expected) = op.attempt_id() {
        let attempt = item
            .latest_attempt
            .as_ref()
            .ok_or(WorkError::AttemptConflict)?;
        if attempt.work_attempt_id != expected
            || attempt.closure.is_some()
            || attempt.claimant != *actor
            || (matches!(
                op,
                WorkOperation::Block { .. }
                    | WorkOperation::Resume { .. }
                    | WorkOperation::Complete { .. }
            ) && attempt.executor.as_ref() != Some(actor))
        {
            return Err(WorkError::AttemptConflict);
        }
    }
    let risk = matches!(
        op,
        WorkOperation::Cancel { .. } | WorkOperation::Requeue { .. }
    ) && matches!(
        item.state,
        WorkState::Claimed | WorkState::Working | WorkState::Blocked
    );
    match op {
        WorkOperation::Post { .. } => (),
        WorkOperation::Claim { .. } => {
            item.attempt_count = checked_increment(item.attempt_count)?;
            item.latest_attempt = Some(WorkAttempt {
                work_attempt_id: format!("work_attempt_{}", ulid::Ulid::new()),
                work_id: item.work_id.clone(),
                number: item.attempt_count,
                claimant: actor.clone(),
                claimed_at: at.into(),
                executor: None,
                started_at: None,
                ended_at: None,
                closure: None,
                claim_event_id: event_id.into(),
                start_event_id: None,
                terminal_event_id: None,
            });
            item.state = WorkState::Claimed;
        }
        WorkOperation::Start { .. } => {
            let attempt = item
                .latest_attempt
                .as_mut()
                .ok_or(WorkError::AttemptConflict)?;
            attempt.executor = Some(actor.clone());
            attempt.started_at = Some(at.into());
            attempt.start_event_id = Some(event_id.into());
            item.state = WorkState::Working;
        }
        WorkOperation::Block { .. } => item.state = WorkState::Blocked,
        WorkOperation::Resume { .. } => item.state = WorkState::Working,
        _ => {
            let (state, closure) = match op {
                WorkOperation::Complete { .. } => {
                    (WorkState::Completed, WorkAttemptClosure::Completed)
                }
                WorkOperation::Fail { .. } => (WorkState::Failed, WorkAttemptClosure::Failed),
                WorkOperation::Cancel { .. } => {
                    (WorkState::Cancelled, WorkAttemptClosure::Cancelled)
                }
                WorkOperation::Requeue { .. } => (WorkState::Queued, WorkAttemptClosure::Requeued),
                _ => unreachable!(),
            };
            if let Some(attempt) = &mut item.latest_attempt {
                if attempt.closure.is_none() {
                    attempt.closure = Some(closure);
                    attempt.ended_at = Some(at.into());
                    attempt.terminal_event_id = Some(event_id.into());
                }
            }
            item.state = state;
        }
    }
    Ok(risk)
}

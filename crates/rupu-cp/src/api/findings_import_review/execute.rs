use super::{
    discovery, engine,
    model::*,
    store::{Reservation, Store},
};
use crate::error::{ApiError, ApiResult};
use rupu_coverage::tools::{
    attach_prepared_reports, AttachOutcome, PreparedAttachItem, PreparedImportError,
};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

pub(super) fn read_operation(global: &Path, id: &str) -> ApiResult<Operation> {
    let store = Store::open(global).map_err(engine::io_error)?;
    let stored = store
        .load_operation::<Operation>(id)
        .map_err(engine::io_error)?
        .ok_or_else(|| ApiError::not_found("import operation not found"))?;
    validate_operation(
        &stored.payload,
        id,
        &stored.review_id,
        &stored.review_digest,
        &engine::owner(global)?,
    )?;
    let value = observed(stored.payload);
    engine::bounded(&value)?;
    Ok(value)
}
fn validate_operation(
    op: &Operation,
    id: &str,
    review: &str,
    digest: &str,
    owner: &str,
) -> ApiResult<()> {
    if op.reviewed.review_id != review
        || op.reviewed.review_digest != digest
        || op.version != VERSION
        || op.operation_id != id
        || op.review_id != review
        || op.review_digest != digest
        || op.owner != owner
    {
        return Err(ApiError::conflict(
            "import operation belongs to another review or instance",
        ));
    }
    engine::bounded(op)
}
// Acquiring the store lease proves no matching commit is still running. Do not
// repair its persisted journal on a read or guess whether a ledger was replaced.
fn observed(mut op: Operation) -> Operation {
    if matches!(op.status.as_str(), "reserved" | "running") {
        op.status = "unknown".into();
        op.warnings.push(
            "Import stopped without a complete durable receipt. No request was retried.".into(),
        );
    }
    op
}

pub(super) fn commit_at(global: &Path, body: CommitRequest) -> ApiResult<Operation> {
    let store = Store::open(global).map_err(engine::io_error)?;
    let owner = engine::owner(global)?;
    if let Some(existing) = store
        .load_operation::<Operation>(&body.operation_id)
        .map_err(engine::io_error)?
    {
        validate_operation(
            &existing.payload,
            &body.operation_id,
            &body.review_id,
            &body.review_digest,
            &owner,
        )?;
        return Ok(observed(existing.payload));
    }
    let stored = store
        .load_review::<Manifest>(&body.review_id)
        .map_err(engine::io_error)?;
    let manifest = stored.payload;
    let now = chrono::Utc::now().timestamp();
    if manifest.version != VERSION
        || manifest.review_id != body.review_id
        || stored.review_id != body.review_id
        || manifest.owner != owner
        || stored.digest != body.review_digest
        || engine::digest(&manifest)? != body.review_digest
        || manifest.expires_at <= now
        || stored.expires_at <= now as u64
    {
        return Err(ApiError::conflict("import review is expired, changed or belongs to another instance; prepare a new review"));
    }
    let (options, policy) = engine::options(global)?;
    if policy != manifest.policy_digest {
        return Err(ApiError::conflict(
            "findings policy changed; prepare a new review",
        ));
    }
    if manifest.rows.len() > discovery::MAX_REPORTS
        || !manifest.rows.iter().any(|r| r.row.status == "ready")
    {
        return Err(ApiError::bad_request("review has no eligible report set"));
    }
    let stage = store
        .review_path(&body.review_id)
        .map_err(engine::io_error)?;
    let mut groups = BTreeMap::<PathBuf, Vec<usize>>::new();
    for (index, row) in manifest.rows.iter().enumerate() {
        if row.row.status == "ready" {
            let Some(target) = row.row.target.as_ref() else {
                return Err(ApiError::conflict("invalid reviewed target"));
            };
            if row.prepared.is_none() || row.prepared_ledger.is_none() {
                return Err(ApiError::conflict("reviewed content is missing"));
            }
            groups
                .entry(target.ledger_path.clone())
                .or_default()
                .push(index);
        }
    }
    let mut op = Operation {
        version: VERSION,
        reviewed: engine::preview(&manifest, body.review_digest.clone())?,
        operation_id: body.operation_id.clone(),
        review_id: body.review_id.clone(),
        review_digest: body.review_digest.clone(),
        owner,
        status: "reserved".into(),
        receipt_durable: true,
        rows: manifest
            .rows
            .iter()
            .map(|r| ResultRow {
                ordinal: r.row.ordinal,
                source_path: r.row.source_path.clone(),
                target: r.row.target.clone(),
                status: if r.row.status == "ready" {
                    "not_attempted".into()
                } else {
                    r.row.status.clone()
                },
                reason: r.row.reason.clone(),
            })
            .collect(),
        ledgers: groups
            .iter()
            .map(|(path, indices)| {
                let target = manifest.rows[indices[0]]
                    .row
                    .target
                    .as_ref()
                    .expect("checked target");
                LedgerResult {
                    ledger_path: path.clone(),
                    ws_id: target.ws_id.clone(),
                    target_id: target.target_id.clone(),
                    status: "not_attempted".into(),
                    backup: None,
                    artifact_side_effects_possible: false,
                }
            })
            .collect(),
        warnings: Vec::new(),
    };
    engine::bounded(&op)?;
    // Retain room for every bounded diagnostic and backup after mutation;
    // never discover response capacity only after a ledger was replaced.
    if serde_json::to_vec(&op)
        .map_err(|_| ApiError::internal("cannot encode import reservation"))?
        .len()
        > MAX_RESPONSE_BYTES - 768 * 1024
    {
        return Err(ApiError::bad_request(
            "review leaves insufficient receipt space; select fewer reports",
        ));
    }
    match store
        .reserve_operation(
            &body.operation_id,
            &body.review_id,
            &body.review_digest,
            op.clone(),
        )
        .map_err(engine::io_error)?
    {
        Reservation::Existing(previous) => return Ok(observed(previous.payload)),
        Reservation::Reserved(_) => {}
    }
    // Failure here is known no-effect, and is still retained as an operation.
    let current = match discovery::discover_ledgers(global) {
        Ok(value) => value,
        Err(_) => {
            op.status = "refused".into();
            op.warnings
                .push("Target discovery is incomplete; no finding was changed.".into());
            return Ok(publish_or_uncertain(&store, op));
        }
    };
    for (ledger_index, (_, indices)) in groups.into_iter().enumerate() {
        let target = manifest.rows[indices[0]]
            .row
            .target
            .as_ref()
            .expect("checked target");
        if !indices.iter().all(|i| {
            engine::target_current(
                manifest.rows[*i]
                    .row
                    .target
                    .as_ref()
                    .expect("checked target"),
                &current,
            )
        }) {
            for &i in &indices {
                op.rows[i].status = "conflict".into();
                op.rows[i].reason =
                    Some("reviewed target or ledger changed; prepare a new review".into());
            }
            op.ledgers[ledger_index].status = "conflict".into();
            if !publish_progress(&store, &mut op) {
                return Ok(op);
            }
            continue;
        }
        // Recheck the selected registration immediately before mutation. The
        // captured target path is never redirected by fresh ID resolution.
        let registry = discovery::read_regular_no_follow(&target.registry_path, 32 * 1024);
        if !registry.is_ok_and(|r| {
            r.sha256 == target.registry_sha256 && r.identity == target.registry_identity
        }) {
            for &i in &indices {
                op.rows[i].status = "conflict".into();
                op.rows[i].reason = Some("project registration changed".into());
            }
            op.ledgers[ledger_index].status = "conflict".into();
            if !publish_progress(&store, &mut op) {
                return Ok(op);
            }
            continue;
        }
        // This boundary is durable BEFORE any artifact install/ledger write.
        op.status = "running".into();
        for &i in &indices {
            op.rows[i].status = "unknown".into();
            op.rows[i].reason = Some("ledger operation has no confirmed receipt yet".into());
        }
        op.ledgers[ledger_index].status = "unknown".into();
        op.ledgers[ledger_index].artifact_side_effects_possible = true;
        if !publish_progress(&store, &mut op) {
            return Ok(op);
        }
        let items: Vec<_> = indices
            .iter()
            .map(|i| PreparedAttachItem {
                finding_id: manifest.rows[*i]
                    .row
                    .target
                    .as_ref()
                    .expect("checked target")
                    .finding_id
                    .clone(),
                prepared: manifest.rows[*i].prepared.clone().expect("checked content"),
            })
            .collect();
        let paths = rupu_coverage::CoveragePaths::new(&target.workspace_path, &target.target_id);
        match attach_prepared_reports(
            &paths,
            &items,
            &options,
            &target.ledger_sha256,
            manifest.rows[indices[0]]
                .prepared_ledger
                .as_ref()
                .expect("checked ledger"),
            &stage.join("artifacts"),
        ) {
            Ok(batch) => {
                if batch.outcomes.len() != indices.len() {
                    return Ok(op);
                }
                op.ledgers[ledger_index].backup = batch.backup;
                op.ledgers[ledger_index].status = if batch.write_error.is_some() {
                    "failed"
                } else {
                    "completed"
                }
                .into();
                for (&i, result) in indices.iter().zip(batch.outcomes) {
                    let (status, reason) = match result {
                        AttachOutcome::Attached => ("attached", None),
                        AttachOutcome::AlreadyHasReport => (
                            "skipped",
                            Some("a full report is already recorded; it was not replaced".into()),
                        ),
                        AttachOutcome::NotFound => (
                            "conflict",
                            Some("finding is absent from its reviewed ledger".into()),
                        ),
                        AttachOutcome::Duplicate => (
                            "conflict",
                            Some("finding is duplicated in its reviewed ledger".into()),
                        ),
                        AttachOutcome::Rejected(e) => {
                            ("failed", Some(engine::reason_text(&e.to_string())))
                        }
                        AttachOutcome::NotWritten => (
                            "failed",
                            Some("ledger replacement was not confirmed by the writer".into()),
                        ),
                    };
                    op.rows[i].status = status.into();
                    op.rows[i].reason = reason;
                }
            }
            Err(PreparedImportError::LedgerChanged) => {
                op.ledgers[ledger_index].status = "conflict".into();
                op.ledgers[ledger_index].artifact_side_effects_possible = false;
                for &i in &indices {
                    op.rows[i].status = "conflict".into();
                    op.rows[i].reason = Some("reviewed ledger identity or contents changed".into());
                }
            }
            Err(PreparedImportError::Unconfirmed { backup, reason }) => {
                op.ledgers[ledger_index].backup = backup;
                op.status = "unknown".into();
                op.warnings.push(engine::reason_text(&reason));
                op.warnings.push("The ledger may have changed; its backup is retained. Remaining ledgers were not attempted.".into());
                return Ok(publish_or_uncertain(&store, op));
            }
            Err(_) => {
                // Includes post-rename fsync/IO failures: never label unknown as
                // a safe failure, and never retry or start another ledger.
                op.status = "unknown".into();
                op.warnings.push("A ledger operation could not be confirmed. Remaining ledgers were not attempted.".into());
                return Ok(publish_or_uncertain(&store, op));
            }
        }
        if !publish_progress(&store, &mut op) {
            return Ok(op);
        }
    }
    op.status = "completed".into(); // All rows have known outcomes; may be mixed.
    Ok(publish_or_uncertain(&store, op))
}
fn publish_progress(store: &Store, op: &mut Operation) -> bool {
    let terminal = matches!(op.status.as_str(), "completed" | "refused")
        && op.rows.iter().all(|row| row.status != "unknown");
    let saved = if engine::bounded(op).is_err() {
        false
    } else if terminal {
        store.finish_operation(&op.operation_id, &*op).is_ok()
    } else {
        store.update_operation(&op.operation_id, &*op).is_ok()
    };
    if !saved {
        op.receipt_durable = false;
        op.status = "unknown".into();
        op.warnings.push("This response retains known outcomes, but the latest receipt was not durably saved. No further ledger was attempted.".into());
        false
    } else {
        true
    }
}
fn publish_or_uncertain(store: &Store, mut op: Operation) -> Operation {
    publish_progress(store, &mut op);
    op
}

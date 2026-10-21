//! One run's handle on its message space (spec F1a): the space's log and
//! participant registry, the board and mailboxes it shares with every other
//! participant, who this run is in it, and the claims it holds.
//!
//! A [`Bus`] is per run, because its `participant`, its member record and
//! its [`ClaimTable`] are per run; the stores it points at are shared by
//! every participant of the space. Every write goes through one writer API
//! that appends to `log.jsonl` under `log.jsonl.lock` BEFORE it touches the
//! working state (board files, inboxes), so the log is always a superset: a
//! crash between the two leaves a logged message nobody received (and no
//! `seen` line says otherwise), never a received message the log lacks.

use crate::address::{self, Address};
use crate::board::Board;
use crate::error::{BusError, FleetError};
use crate::log::{
    self, Delivery, LogLine, LogMessage, LogPage, MessageKind, OperatorVia, Refs, SendReceipt,
    Undelivered,
};
use crate::mailbox::Mailbox;
use crate::participants::{
    Member, Participant, ParticipantEvent, Resolution, Roster, Sender, Undeliverable,
};
use crate::types::{BoardPost, ClaimGuard, Directive, FleetMessage, PostKind};
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, RwLock};

use std::time::Duration;

/// The default `[messaging].log_max_mb`.
pub const DEFAULT_LOG_MAX_MB: u64 = 64;

/// Limits a bus applies to what its run writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BusCaps {
    /// Per-participant inbox cap for direct messages (for `broadcast`: the
    /// cap on the whole broadcast log). Backpressure: a message to a full
    /// inbox is logged as undelivered.
    pub msg_cap: usize,
    /// The lease a `board.claim` takes on a work unit.
    pub claim_ttl: Duration,
    /// The size past which the log takes no more messages (FD3): a send
    /// then fails to its sender. Receipts are still written.
    pub log_max_bytes: u64,
}

impl Default for BusCaps {
    fn default() -> Self {
        Self {
            msg_cap: 256,
            claim_ttl: Duration::from_secs(3600),
            log_max_bytes: DEFAULT_LOG_MAX_MB * 1024 * 1024,
        }
    }
}

/// The claims one run holds, keyed by work unit.
///
/// A [`ClaimGuard`] releases its claim when dropped, so a stateless tool call
/// cannot simply return it: granted guards are kept here for as long as the
/// claim should stand. Removing one (`board.release`) — or dropping the whole
/// table when the run's bus goes away — releases it.
#[derive(Debug, Default)]
pub struct ClaimTable(Mutex<HashMap<String, ClaimGuard>>);

impl ClaimTable {
    /// The held guards. A poisoned lock is recovered: the map only ever holds
    /// guards, and a panic elsewhere must not wedge every later claim.
    pub fn lock(&self) -> MutexGuard<'_, HashMap<String, ClaimGuard>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// A message from the operator (spec F1b §3.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperatorMessage {
    pub to: Address,
    pub body: String,
    pub urgent: bool,
    pub via: OperatorVia,
    /// Who sent it, when the surface knows (the CP's viewer).
    pub viewer: Option<String>,
    /// The client behind the surface (`mcp`), recorded beside `via`.
    pub client: Option<String>,
}

/// One run's address in its message space.
#[derive(Debug)]
pub struct Bus {
    pub board: Board,
    pub mailbox: Mailbox,
    /// This run's address: the `from` of what it writes, the inbox others
    /// reach it at (`lead`, `lynx#1`).
    pub participant: String,
    pub claims: ClaimTable,
    pub caps: BusCaps,
    space: PathBuf,
    /// Who the run is in the space, once it joined (a bare bus — tests,
    /// tools in isolation, the operator — never does).
    member: RwLock<Option<Member>>,
    /// Whether the agent loop records the run as `left` when it ends. An
    /// agentiflow lead's rounds and a session's turns stay joined between
    /// runs.
    leave_on_exit: bool,
    #[cfg(test)]
    pub(crate) fail_after_log: std::sync::atomic::AtomicBool,
}

impl Bus {
    /// `participant`'s bus on the space at `space` (an agentiflow's run dir,
    /// or `<run dir>/messages`). Nothing is created until something is
    /// written.
    pub fn open(space: &Path, participant: impl Into<String>) -> Self {
        Self {
            board: Board::new(space),
            mailbox: Mailbox::new(space),
            participant: participant.into(),
            claims: ClaimTable::default(),
            caps: BusCaps::default(),
            space: space.to_path_buf(),
            member: RwLock::new(None),
            leave_on_exit: true,
            #[cfg(test)]
            fail_after_log: Default::default(),
        }
    }

    pub fn with_caps(mut self, caps: BusCaps) -> Self {
        self.caps = caps;
        self
    }

    /// The run stays joined when it ends (an agentiflow lead between
    /// rounds, a session between turns).
    pub fn staying(mut self) -> Self {
        self.leave_on_exit = false;
        self
    }

    pub fn leaves_on_exit(&self) -> bool {
        self.leave_on_exit
    }

    pub fn space(&self) -> &Path {
        &self.space
    }

    pub fn log_path(&self) -> PathBuf {
        self.space.join("log.jsonl")
    }

    pub fn participants_path(&self) -> PathBuf {
        self.space.join("participants.jsonl")
    }

    /// Who the run is, once it joined.
    pub fn member(&self) -> Option<Member> {
        self.member
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Whether the run's `joined` line is written.
    pub fn is_joined(&self) -> bool {
        self.member
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .is_some()
    }

    /// A bus for `participant` on the same space (a child sharing its
    /// parent's), with the same caps and claims of its own.
    pub fn sibling(&self, participant: impl Into<String>) -> Bus {
        Bus::open(&self.space, participant).with_caps(self.caps)
    }

    /// The address of a run below this one, named `codename`
    /// ([`address::below`]). `None` when this run has no codename or
    /// `codename` isn't below it.
    pub fn address_below(&self, codename: &str) -> Option<String> {
        let own = self.member()?.codename?;
        address::below(&own, &self.participant, codename)
    }

    pub fn roster(&self) -> Result<Roster, BusError> {
        Roster::read(&self.participants_path())
    }

    /// Record that this run joined the space as `member`, and start its
    /// broadcast cursor. Idempotent for the same run: an agentiflow lead
    /// joins once, however many rounds it runs. A run can't join at a fixed
    /// address other than `lead`: it would shadow what that address means.
    pub fn join(&self, member: Member) -> Result<(), BusError> {
        if [address::OPERATOR, address::BROADCAST, address::PARENT]
            .contains(&self.participant.as_str())
        {
            return Err(FleetError::Refused(format!(
                "a run can't join as `{}`: it is a fixed address",
                self.participant
            )));
        }
        let _lock = self.lock()?;
        let roster = self.roster()?;
        let rejoin = roster
            .get(&self.participant)
            .is_some_and(|p| p.live && p.member == member);
        if !rejoin {
            append(
                &self.participants_path(),
                &ParticipantEvent::Joined {
                    address: self.participant.clone(),
                    member: member.clone(),
                    at: now(),
                },
            )?;
        }
        self.mailbox.register_broadcast_cursor(&self.participant)?;
        *self.member.write().unwrap_or_else(|e| e.into_inner()) = Some(member);
        Ok(())
    }

    /// Record that this run left the space with `status`. A run that never
    /// joined writes nothing.
    pub fn leave(&self, status: &str) -> Result<(), BusError> {
        let Some(member) = self.member() else {
            return Ok(());
        };
        let _lock = self.lock()?;
        append(
            &self.participants_path(),
            &ParticipantEvent::Left {
                address: self.participant.clone(),
                run_id: member.run_id,
                status: status.to_string(),
                at: now(),
            },
        )
    }

    /// Record that every participant still live in the space at `space`
    /// left with `status`: its runs ended without leaving (a session stopped
    /// or archived between turns). Returns how many it recorded; a space
    /// with no participants is left unwritten.
    pub fn leave_all(space: &Path, status: &str) -> Result<usize, BusError> {
        let bus = Bus::open(space, address::OPERATOR);
        if !bus.participants_path().exists() {
            return Ok(0);
        }
        let _lock = bus.lock()?;
        let live: Vec<Participant> = bus
            .roster()?
            .participants()
            .filter(|p| p.live)
            .cloned()
            .collect();
        let at = now();
        for p in &live {
            append(
                &bus.participants_path(),
                &ParticipantEvent::Left {
                    address: p.address.clone(),
                    run_id: p.member.run_id.clone(),
                    status: status.to_string(),
                    at: at.clone(),
                },
            )?;
        }
        Ok(live.len())
    }

    /// A board post, optionally addressed. Every participant can read it on
    /// the board. An addressed post is also delivered to each resolved
    /// addressee's inbox (an @mention reaches the one it names), under the
    /// same inbox cap as a direct message; an unaddressed post is a channel
    /// post and reaches nobody's inbox. A post to `broadcast` is a channel
    /// post (there is one broadcast, `send`'s).
    pub fn post(
        &self,
        kind: PostKind,
        to: Option<&Address>,
        body: &str,
    ) -> Result<SendReceipt, BusError> {
        let to = to.filter(|a| **a != Address::Broadcast);
        self.write(
            Draft {
                kind: MessageKind::Post,
                to,
                post_kind: Some(kind),
                body,
                refs: Refs::default(),
                ..Draft::plain()
            },
            |m| {
                self.board.post(&BoardPost {
                    id: Some(m.id.clone()),
                    author: m.from.clone(),
                    ts: m.ts.clone(),
                    kind,
                    body: m.body.clone(),
                    addressed_to: m.to.clone(),
                })?;
                if m.to.is_some() {
                    self.deliver(m, false)?;
                }
                Ok(())
            },
        )
    }

    /// A message to a participant, a role, a step, the parent, the lead or
    /// the operator (each resolved recipient's inbox), or to `broadcast`
    /// (the broadcast log).
    pub fn send(&self, to: &Address, body: &str) -> Result<SendReceipt, BusError> {
        let kind = match to {
            Address::Broadcast => MessageKind::Broadcast,
            _ => MessageKind::Direct,
        };
        self.write(
            Draft {
                kind,
                to: Some(to),
                post_kind: None,
                body,
                refs: Refs::default(),
                ..Draft::plain()
            },
            |m| self.deliver(m, kind == MessageKind::Broadcast),
        )
    }

    /// A message from the operator (spec F1b §3.1): a `kind: operator` log
    /// line from `operator`, delivered as `send` delivers. The operator
    /// never joins; no catalog tool reaches this, so an agent can never
    /// speak as the operator (FD5).
    pub fn operator_send(&self, msg: OperatorMessage) -> Result<SendReceipt, BusError> {
        if msg.to == Address::Operator {
            return Err(FleetError::Refused(
                "the operator can't send a message to the operator".into(),
            ));
        }
        let broadcast = msg.to == Address::Broadcast;
        self.write_as(
            address::OPERATOR,
            Draft {
                kind: MessageKind::Operator,
                to: Some(&msg.to),
                post_kind: None,
                body: &msg.body,
                refs: Refs::default(),
                urgent: msg.urgent,
                via: Some(msg.via),
                viewer: msg.viewer.clone(),
                client: msg.client.clone(),
            },
            |m| self.deliver(m, broadcast),
        )
    }

    /// Put the logged message `m` where its recipients read it: the
    /// broadcast log, or each resolved recipient's inbox.
    fn deliver(&self, m: &LogMessage, broadcast: bool) -> Result<(), FleetError> {
        let line = FleetMessage {
            id: Some(m.id.clone()),
            from: m.from.clone(),
            from_codename: m.from_codename.clone(),
            to: m.to.clone(),
            ts: m.ts.clone(),
            body: m.body.clone(),
            urgent: m.urgent,
            post_kind: m.post_kind,
        };
        if broadcast {
            if m.delivery.undelivered.is_empty() {
                self.mailbox.broadcast_send(&line, self.caps.msg_cap)?;
            }
            return Ok(());
        }
        for r in &m.delivery.resolved {
            // The operator's messages are read from the log.
            if r != address::OPERATOR {
                self.mailbox.send(r, &line, self.caps.msg_cap)?;
            }
        }
        Ok(())
    }

    /// A standing directive, to everyone (`None`, or `broadcast`) or to an
    /// address. Its id is its log id, which `retract` takes.
    pub fn directive(&self, to: Option<&Address>, body: &str) -> Result<SendReceipt, BusError> {
        let to = to.filter(|a| **a != Address::Broadcast);
        self.write(
            Draft {
                kind: MessageKind::Directive,
                to,
                post_kind: None,
                body,
                refs: Refs::default(),
                ..Draft::plain()
            },
            |m| {
                self.board
                    .put_directive(&Directive {
                        id: m.id.clone(),
                        author: m.from.clone(),
                        ts: m.ts.clone(),
                        body: m.body.clone(),
                        addressed_to: m.to.clone(),
                    })
                    .map(drop)
            },
        )
    }

    /// Lift the directive `directive` (authorship is the caller's to check).
    pub fn retract(&self, directive: &str) -> Result<SendReceipt, BusError> {
        self.write(
            Draft {
                kind: MessageKind::Retract,
                to: None,
                post_kind: None,
                body: "",
                refs: Refs {
                    retracts: Some(directive.to_string()),
                    reply_to: None,
                },
                ..Draft::plain()
            },
            |_| self.board.retract_directive(directive),
        )
    }

    /// Record that `by` saw the messages `ids` on its turn `turn`. Written
    /// past the size cap too: a receipt is never refused.
    pub fn mark_seen(&self, ids: &[String], by: &str, turn: u32) -> Result<(), BusError> {
        if ids.is_empty() {
            return Ok(());
        }
        let _lock = self.lock()?;
        let at = now();
        for id in ids {
            append(
                &self.log_path(),
                &LogLine::Seen {
                    id: id.clone(),
                    by: by.to_string(),
                    turn,
                    at: at.clone(),
                },
            )?;
        }
        Ok(())
    }

    /// The log after the message `after`, oldest first. A space with no log
    /// (an agentiflow from before it existed) reads as empty.
    pub fn read_log(&self, after: Option<&str>, limit: usize) -> Result<LogPage, BusError> {
        Ok(log::page(log::read_lines(&self.log_path())?, after, limit))
    }

    fn lock(&self) -> Result<std::fs::File, BusError> {
        std::fs::create_dir_all(&self.space).map_err(|e| FleetError::Io {
            action: format!("create message space {}", self.space.display()),
            source: e,
        })?;
        lock_exclusive(&self.space.join("log.jsonl.lock"))
    }

    /// A write by this run. `from: operator` is the operator's alone
    /// ([`Bus::operator_send`]): a bus at that address writes nothing else,
    /// so a log line from `operator` is always the operator's.
    fn write(
        &self,
        draft: Draft<'_>,
        apply: impl FnOnce(&LogMessage) -> Result<(), FleetError>,
    ) -> Result<SendReceipt, BusError> {
        if self.participant == address::OPERATOR {
            return Err(FleetError::Refused(
                "a run can't write as `operator`: the operator's messages go through operator_send"
                    .into(),
            ));
        }
        self.write_as(&self.participant, draft, apply)
    }

    /// The one write path: under the log lock, check the size cap, resolve
    /// the address, append the message, then update the working state.
    fn write_as(
        &self,
        from: &str,
        draft: Draft<'_>,
        apply: impl FnOnce(&LogMessage) -> Result<(), FleetError>,
    ) -> Result<SendReceipt, BusError> {
        let _lock = self.lock()?;
        let log_path = self.log_path();
        let size = std::fs::metadata(&log_path).map_or(0, |m| m.len());
        if size >= self.caps.log_max_bytes {
            return Err(FleetError::LogFull {
                max_bytes: self.caps.log_max_bytes,
            });
        }
        let member = self.member();
        let (to, delivery) = match draft.to {
            None => (None, self.unaddressed(draft.kind)?),
            Some(to) => {
                let roster = self.roster()?;
                let resolution = roster.resolve(
                    to,
                    Sender {
                        address: from,
                        parent_run_id: member.as_ref().and_then(|m| m.parent_run_id.as_deref()),
                    },
                    &|name| self.mailbox.has_mailbox(name),
                );
                let to_string = resolution.to.clone();
                (
                    Some(to_string),
                    self.delivery(draft.kind, &draft.to, resolution)?,
                )
            }
        };
        let message = LogMessage {
            id: format!("msg_{}", ulid::Ulid::new()),
            ts: now(),
            kind: draft.kind,
            from: from.to_string(),
            from_codename: member.and_then(|m| m.codename),
            to,
            post_kind: draft.post_kind,
            body: draft.body.to_string(),
            urgent: draft.urgent,
            refs: draft.refs,
            delivery,
            via: draft.via,
            viewer: draft.viewer,
            client: draft.client,
        };
        append(&log_path, &LogLine::Msg(message.clone()))?;
        #[cfg(test)]
        let apply = |m: &LogMessage| {
            if self
                .fail_after_log
                .load(std::sync::atomic::Ordering::SeqCst)
            {
                return Err(FleetError::Io {
                    action: "injected failure after the log line".into(),
                    source: std::io::Error::other("injected"),
                });
            }
            apply(m)
        };
        apply(&message).map_err(|e| FleetError::PartlyDelivered {
            id: message.id.clone(),
            source: Box::new(e),
        })?;
        Ok(SendReceipt { message })
    }

    /// Who an unaddressed post or directive reaches: everyone live, for a
    /// directive; a channel post names nobody.
    fn unaddressed(&self, kind: MessageKind) -> Result<Delivery, BusError> {
        if kind != MessageKind::Directive {
            return Ok(Delivery::default());
        }
        Ok(Delivery {
            resolved: self
                .roster()?
                .participants()
                .filter(|p| p.live && p.address != self.participant)
                .map(|p| p.address.clone())
                .collect(),
            undelivered: Vec::new(),
        })
    }

    /// `resolution` with the backpressure caps applied: a recipient whose
    /// inbox is full, or a full broadcast log, is undelivered.
    fn delivery(
        &self,
        kind: MessageKind,
        to: &Option<&Address>,
        resolution: Resolution,
    ) -> Result<Delivery, BusError> {
        // An operator message travels as a broadcast or a direct message.
        let kind = match kind {
            MessageKind::Operator if *to == Some(&Address::Broadcast) => MessageKind::Broadcast,
            MessageKind::Operator => MessageKind::Direct,
            kind => kind,
        };
        let mut undelivered: Vec<Undelivered> = resolution
            .undelivered
            .map(|reason| Undelivered {
                to: resolution.to.clone(),
                reason,
            })
            .into_iter()
            .collect();
        let mut resolved = Vec::new();
        match kind {
            MessageKind::Broadcast => {
                if self.mailbox.broadcast_len()? >= self.caps.msg_cap {
                    undelivered.push(Undelivered {
                        to: resolution.to,
                        reason: Undeliverable::BroadcastFull,
                    });
                } else {
                    resolved = resolution.recipients;
                }
            }
            // An addressed post lands in its addressees' inboxes too.
            MessageKind::Direct | MessageKind::Post => {
                for r in resolution.recipients {
                    if r != address::OPERATOR && self.mailbox.inbox_len(&r)? >= self.caps.msg_cap {
                        undelivered.push(Undelivered {
                            to: r,
                            reason: Undeliverable::InboxFull,
                        });
                    } else {
                        resolved.push(r);
                    }
                }
            }
            _ => resolved = resolution.recipients,
        }
        Ok(Delivery {
            resolved,
            undelivered,
        })
    }
}

struct Draft<'a> {
    kind: MessageKind,
    to: Option<&'a Address>,
    post_kind: Option<PostKind>,
    body: &'a str,
    refs: Refs,
    urgent: bool,
    via: Option<OperatorVia>,
    viewer: Option<String>,
    client: Option<String>,
}

impl Draft<'_> {
    /// The fields only an operator message sets, unset.
    fn plain() -> Self {
        Draft {
            kind: MessageKind::Post,
            to: None,
            post_kind: None,
            body: "",
            refs: Refs::default(),
            urgent: false,
            via: None,
            viewer: None,
            client: None,
        }
    }
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Append one JSON line with a single `write_all` on an `O_APPEND` handle.
fn append<T: serde::Serialize>(path: &Path, value: &T) -> Result<(), FleetError> {
    let mut line = serde_json::to_vec(value)?;
    line.push(b'\n');
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| FleetError::Io {
            action: format!("open {}", path.display()),
            source: e,
        })?;
    f.write_all(&line).map_err(|e| FleetError::Io {
        action: format!("append {}", path.display()),
        source: e,
    })
}

/// An exclusive `flock` on `path`, held while the returned file lives.
pub(crate) fn lock_exclusive(path: &Path) -> Result<std::fs::File, FleetError> {
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)
        .map_err(|e| FleetError::Io {
            action: format!("open {}", path.display()),
            source: e,
        })?;
    loop {
        match rustix::fs::flock(&lock, rustix::fs::FlockOperation::LockExclusive) {
            Ok(()) => return Ok(lock),
            Err(rustix::io::Errno::INTR) => continue,
            Err(e) => {
                return Err(FleetError::Io {
                    action: format!("lock {}", path.display()),
                    source: e.into(),
                })
            }
        }
    }
}

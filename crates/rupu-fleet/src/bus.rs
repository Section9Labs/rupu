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
    self, Delivery, LogLine, LogMessage, LogPage, MessageKind, Refs, SendReceipt, Undelivered,
};
use crate::mailbox::Mailbox;
use crate::participants::{Member, ParticipantEvent, Resolution, Roster, Sender, Undeliverable};
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
    /// Set by [`Bus::join`]: the run's codename, agent, step and parent.
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
    /// joins once, however many rounds it runs.
    pub fn join(&self, member: Member) -> Result<(), BusError> {
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
        *self.member.write().unwrap_or_else(|e| e.into_inner()) = Some(member);
        self.mailbox.register_broadcast_cursor(&self.participant)
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

    /// A board post, optionally addressed. Every participant can read it;
    /// nothing delivers it into a turn.
    pub fn post(
        &self,
        kind: PostKind,
        to: Option<&Address>,
        body: &str,
    ) -> Result<SendReceipt, BusError> {
        self.write(
            Draft {
                kind: MessageKind::Post,
                to,
                post_kind: Some(kind),
                body,
                refs: Refs::default(),
            },
            |m| {
                self.board.post(&BoardPost {
                    id: Some(m.id.clone()),
                    author: m.from.clone(),
                    ts: m.ts.clone(),
                    kind,
                    body: m.body.clone(),
                    addressed_to: m.to.clone(),
                })
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
            },
            |m| {
                let line = FleetMessage {
                    id: Some(m.id.clone()),
                    from: m.from.clone(),
                    from_codename: m.from_codename.clone(),
                    to: m.to.clone(),
                    ts: m.ts.clone(),
                    body: m.body.clone(),
                };
                if kind == MessageKind::Broadcast {
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
            },
        )
    }

    /// A standing directive, to everyone (`None`) or to an address. Its id
    /// is its log id, which `retract` takes.
    pub fn directive(&self, to: Option<&Address>, body: &str) -> Result<SendReceipt, BusError> {
        self.write(
            Draft {
                kind: MessageKind::Directive,
                to,
                post_kind: None,
                body,
                refs: Refs::default(),
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

    /// The one write path: under the log lock, check the size cap, resolve
    /// the address, append the message, then update the working state.
    fn write(
        &self,
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
                        address: &self.participant,
                        parent_run_id: member.as_ref().and_then(|m| m.parent_run_id.as_deref()),
                    },
                    &|name| self.mailbox.has_mailbox(name),
                );
                let to_string = resolution.to.clone();
                (Some(to_string), self.delivery(draft.kind, resolution)?)
            }
        };
        let message = LogMessage {
            id: format!("msg_{}", ulid::Ulid::new()),
            ts: now(),
            kind: draft.kind,
            from: self.participant.clone(),
            from_codename: member.and_then(|m| m.codename),
            to,
            post_kind: draft.post_kind,
            body: draft.body.to_string(),
            urgent: false,
            refs: draft.refs,
            delivery,
        };
        append(&log_path, &LogLine::Msg(message.clone()))?;
        #[cfg(test)]
        if self
            .fail_after_log
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            return Err(FleetError::Io {
                action: "injected failure after the log line".into(),
                source: std::io::Error::other("injected"),
            });
        }
        apply(&message)?;
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
    fn delivery(&self, kind: MessageKind, resolution: Resolution) -> Result<Delivery, BusError> {
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
            MessageKind::Direct => {
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
fn lock_exclusive(path: &Path) -> Result<std::fs::File, FleetError> {
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

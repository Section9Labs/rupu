//! One run's handle on its agentiflow's message substrate: the shared board,
//! the mailboxes, who this run is on them, and the claims it holds.
//!
//! A [`Bus`] is per run, because its `participant` and its [`ClaimTable`]
//! are per run; the [`Board`] and [`Mailbox`] stores it points at are shared
//! by every participant of the flow (they are file-backed under the flow's run
//! directory). The `board.*` / `msg.send` catalog tools act through it
//! (`rupu-tools`, spec W5 §3.2).

use crate::board::Board;
use crate::mailbox::Mailbox;
use crate::types::ClaimGuard;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

/// Per-run limits a bus applies to what its run writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BusCaps {
    /// Per-participant inbox cap for `msg.send` (for `"broadcast"`: the cap
    /// on the whole shared log).
    pub msg_cap: usize,
    /// The lease a `board.claim` takes on a work unit.
    pub claim_ttl: Duration,
}

impl Default for BusCaps {
    fn default() -> Self {
        Self {
            msg_cap: 256,
            claim_ttl: Duration::from_secs(3600),
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

/// One run's address on its flow's board and mailboxes.
#[derive(Debug)]
pub struct Bus {
    pub board: Board,
    pub mailbox: Mailbox,
    /// This run's address: the author of what it posts, the inbox others
    /// reach it at (`lead`, `recon#1`).
    pub participant: String,
    pub claims: ClaimTable,
    pub caps: BusCaps,
}

impl Bus {
    /// `participant`'s bus on the flow whose run directory is `run_dir`. The
    /// board and mailboxes append their own `board/` / `mailboxes/`
    /// subdirectories, so the run directory root is passed as is.
    pub fn for_run_dir(run_dir: &Path, participant: impl Into<String>) -> Self {
        Self::new(Board::new(run_dir), Mailbox::new(run_dir), participant)
    }

    pub fn new(board: Board, mailbox: Mailbox, participant: impl Into<String>) -> Self {
        Self {
            board,
            mailbox,
            participant: participant.into(),
            claims: ClaimTable::default(),
            caps: BusCaps::default(),
        }
    }

    pub fn with_caps(mut self, caps: BusCaps) -> Self {
        self.caps = caps;
        self
    }
}

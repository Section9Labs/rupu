//! What a unit joining an agentiflow brings to its run (`rupu run
//! --fleet-run-dir <dir> --fleet-participant <p>`): the fleet coordination
//! tools, the participant's ambient-context collectors, and the agentiflow's
//! findings scope. Moved here from `rupu-cli` (W3, P9); the run assembler
//! takes them as the launch's injected tools and collectors
//! (`rupu_runtime::assembly::Origin::FlowUnit`).

use std::path::Path;
use std::sync::Arc;

/// The services a fleet unit's run gets from its agentiflow.
pub struct FlowUnitServices {
    /// `board.claim` / `board.release` / `board.post` / `board.read` /
    /// `msg.send`, bound to the run's participant id (until W5 moves them
    /// into the catalog).
    pub extra_tools: Vec<Arc<dyn rupu_tools::Tool>>,
    /// The participant's inbox and the board's standing directives.
    pub collectors: Vec<Arc<dyn rupu_agent::TurnCollector>>,
    /// The agentiflow id — the run dir's basename. Findings pool under
    /// `target_id(workspace, <id>)`, where the goal evaluator looks.
    pub flow_id: String,
}

impl FlowUnitServices {
    /// The services of `participant` in the agentiflow whose run dir is
    /// `run_dir`. The board and mailboxes are the SAME file-backed stores
    /// the lead created there (they append their own `board/` /
    /// `mailboxes/` subdirectories, so the run dir root is passed as is).
    pub fn for_participant(run_dir: &Path, participant: &str) -> Self {
        let board = Arc::new(rupu_fleet::Board::new(run_dir));
        let mailbox = Arc::new(rupu_fleet::Mailbox::new(run_dir));
        let ctx = Arc::new(crate::FleetToolCtx::new(
            Arc::clone(&board),
            Arc::clone(&mailbox),
            participant,
        ));
        Self {
            extra_tools: crate::fleet_tools(ctx),
            collectors: crate::lead_collectors(mailbox, board, participant),
            flow_id: run_dir
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scopes_to_the_run_dir_and_brings_the_board() {
        let tmp = tempfile::tempdir().unwrap();
        let run_dir = tmp.path().join("agentiflows").join("af_01TEST");
        let s = FlowUnitServices::for_participant(&run_dir, "unit-x");
        assert_eq!(s.flow_id, "af_01TEST");
        let names: Vec<_> = s.extra_tools.iter().map(|t| t.name()).collect();
        assert!(names.contains(&"board.post"), "{names:?}");
        assert!(!s.collectors.is_empty());
    }
}

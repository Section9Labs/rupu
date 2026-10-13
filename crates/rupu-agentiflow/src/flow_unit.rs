//! What a unit joining an agentiflow brings to its run (`rupu run
//! --message-space <dir> --participant <p>`): its message bus and the
//! agentiflow's findings scope. The run assembler takes the bus as the
//! launch's (`rupu_runtime::assembly::Origin::FlowUnit`), joins the run to
//! the flow's space with it and attaches the collectors that deliver the
//! unit's messages and directives; the origin's grant (`origin:flow_unit`)
//! offers the board / mailbox catalog tools over it (W5, F1a).

use std::path::Path;
use std::sync::Arc;

/// The services a fleet unit's run gets from its agentiflow.
pub struct FlowUnitServices {
    /// The run's address on the flow's board and mailboxes — `board.claim` /
    /// `board.release` / `board.post` / `board.read` / `msg.send` act
    /// through it as `participant`.
    pub message_bus: Arc<rupu_fleet::Bus>,
    /// The agentiflow id — the run dir's basename. Findings pool under
    /// `target_id(workspace, <id>)`, where the goal evaluator looks.
    pub flow_id: String,
}

impl FlowUnitServices {
    /// The services of `participant` in the agentiflow whose run dir is
    /// `run_dir`. The board and mailboxes are the SAME file-backed stores
    /// the lead created there.
    pub fn for_participant(run_dir: &Path, participant: &str, caps: rupu_fleet::BusCaps) -> Self {
        Self {
            message_bus: Arc::new(rupu_fleet::Bus::open(run_dir, participant).with_caps(caps)),
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
    fn scopes_to_the_run_dir_and_brings_the_bus() {
        let tmp = tempfile::tempdir().unwrap();
        let run_dir = tmp.path().join("agentiflows").join("af_01TEST");
        let s = FlowUnitServices::for_participant(&run_dir, "unit-x", Default::default());
        assert_eq!(s.flow_id, "af_01TEST");
        assert_eq!(s.message_bus.participant, "unit-x");
        assert_eq!(s.message_bus.board.root, run_dir);
        assert_eq!(s.message_bus.space(), run_dir);
    }
}

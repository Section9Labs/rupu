//! Action protocol envelope + step-allowlist validator. Used by the
//! orchestrator (Task 11) to validate `action_emitted` events against
//! a step's `actions:` allowlist.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The shape an agent emits in its `actions[]` array. The runner
/// converts each into a transcript `action_emitted` event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionEnvelope {
    /// The action kind identifier (e.g. `"file_write"`, `"shell_exec"`).
    pub kind: String,
    /// Arbitrary action payload; defaults to JSON null if omitted.
    #[serde(default)]
    pub payload: Value,
}

/// Validates that actions emitted by an agent step are in the step's
/// declared `actions:` allowlist. Real impl lands in Task 11.
pub struct ActionValidator;

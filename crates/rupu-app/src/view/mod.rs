//! GPUI views for the rupu.app main area.
//!
//! Each view is a thin GPUI wrapper around a `rupu-app-canvas`
//! data structure. D-2 ships `graph` (vertical git-graph). D-5 / D-6
//! / D-8 add YAML / Canvas / Transcript.

pub mod drilldown;
pub mod graph;
pub mod launcher;
pub mod transcript_tail;

use gpui::{App, Window};
use std::sync::Arc;

/// Callback type for the "approve" action: receives the step_id of the
/// awaiting step that the user approved.
pub type ApproveCallback = Arc<dyn Fn(String, &mut Window, &mut App) + 'static>;

/// Callback type for the "reject" action: receives (step_id, reason).
pub type RejectCallback = Arc<dyn Fn(String, String, &mut Window, &mut App) + 'static>;

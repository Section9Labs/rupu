//! Launcher — sheet UI for starting a workflow run from inside the
//! app. Owns `LauncherState` (pure data) + the async clone helper.

pub mod clone;
pub mod state;

pub use state::{CloneStatus, LauncherMode, LauncherState, LauncherTarget, ValidationError};

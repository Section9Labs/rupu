//! `write_file` tool — create or overwrite a file. Real impl lands
//! in Task 20 of Plan 1.

/// Writes a file relative to the workspace root, emitting a
/// [`crate::DerivedEvent::FileEdit`]. Implements the [`crate::Tool`]
/// trait in Task 20.
pub struct WriteFileTool;

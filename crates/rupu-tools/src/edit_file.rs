//! `edit_file` tool — exact-match string replacement. Real impl
//! lands in Task 21 of Plan 1.

/// Replaces an exact string in a file relative to the workspace root.
/// Ambiguous matches (more than one occurrence) are an error.
/// Implements the [`crate::Tool`] trait in Task 21.
pub struct EditFileTool;

//! Helpers for building `Attribution` values and dispatching
//! `FileTouchEvent`s to the `CoverageWriter` attached to a `ToolContext`.
//!
//! All helpers are no-ops when `ctx.coverage_writer` is `None`, so
//! there is no overhead in non-coverage runs beyond an `is_none()` check.

use crate::tool::ToolContext;
use rupu_coverage::{Attribution, FileTouchEvent, Surface};

/// Build an `Attribution` from the coverage-related fields on `ctx`.
/// Fields that are not yet wired (Tasks 18/19 will supply real values)
/// fall back to an empty string.
pub fn attribution_from(ctx: &ToolContext) -> Attribution {
    Attribution {
        run_id: ctx.run_id.clone().unwrap_or_default(),
        model: ctx.model.clone().unwrap_or_default(),
        surface: surface_for(ctx),
    }
}

fn surface_for(ctx: &ToolContext) -> Surface {
    match ctx.surface_tag.as_deref() {
        Some("agent") => Surface::Agent,
        Some("autoflow") => Surface::Autoflow,
        Some("session") => Surface::Session,
        _ => Surface::Workflow,
    }
}

/// Emit a `FileTouchEvent` to the writer attached to `ctx`, if any.
/// Silently no-ops when `ctx.coverage_writer` is `None`.
pub async fn emit(ctx: &ToolContext, event: FileTouchEvent) {
    if let Some(writer) = &ctx.coverage_writer {
        writer.record_file_touch(event).await;
    }
}

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

/// If `tool_name` has a mapping in `ctx.tool_mappings` and `input` carries
/// the mapped path arg, return a `FileTouchEvent::Read` for that path.
///
/// Used for unrecognized tools (MCP-provided, custom) that the built-in
/// instrumentation doesn't cover. Returns `None` when:
/// - no `tool_mappings` are loaded on the context, OR
/// - `tool_name` has no entry in the mappings, OR
/// - the mapped `path_arg` key is absent from `input` (or not a string).
pub fn mapped_touch(
    ctx: &ToolContext,
    tool_name: &str,
    input: &serde_json::Value,
) -> Option<rupu_coverage::FileTouchEvent> {
    let mappings = ctx.tool_mappings.as_deref()?;
    let mapping = mappings.get(tool_name)?;
    let path = input.get(&mapping.path_arg)?.as_str()?.to_string();
    Some(rupu_coverage::FileTouchEvent::Read {
        path,
        // Mapped tools don't report a precise range; [0,0] = whole/unknown.
        line_range: [0, 0],
        tool: tool_name.to_string(),
        attribution: attribution_from(ctx),
        at: chrono::Utc::now(),
    })
}

#[cfg(test)]
mod mapped_touch_tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::Arc;

    fn ctx_with_mappings(map: rupu_coverage::ToolMappings) -> ToolContext {
        ToolContext {
            run_id: Some("r".into()),
            model: Some("m".into()),
            surface_tag: Some("workflow".into()),
            tool_mappings: Some(Arc::new(map)),
            ..Default::default()
        }
    }

    #[test]
    fn mapped_tool_with_path_arg_yields_event() {
        let mut tools = BTreeMap::new();
        tools.insert(
            "cat_file".to_string(),
            rupu_coverage::ToolMapping {
                path_arg: "path".into(),
                kind: "read".into(),
            },
        );
        let ctx = ctx_with_mappings(rupu_coverage::ToolMappings { tools });
        let input = serde_json::json!({ "path": "src/x.rs" });
        let ev = mapped_touch(&ctx, "cat_file", &input).expect("should map");
        assert_eq!(ev.path(), Some("src/x.rs"));
    }

    #[test]
    fn unmapped_tool_yields_none() {
        let ctx = ctx_with_mappings(rupu_coverage::ToolMappings::default());
        let input = serde_json::json!({ "path": "src/x.rs" });
        assert!(mapped_touch(&ctx, "cat_file", &input).is_none());
    }

    #[test]
    fn mapped_tool_missing_arg_yields_none() {
        let mut tools = BTreeMap::new();
        tools.insert(
            "cat_file".to_string(),
            rupu_coverage::ToolMapping {
                path_arg: "path".into(),
                kind: "read".into(),
            },
        );
        let ctx = ctx_with_mappings(rupu_coverage::ToolMappings { tools });
        let input = serde_json::json!({ "other": "x" });
        assert!(mapped_touch(&ctx, "cat_file", &input).is_none());
    }

    #[test]
    fn no_mappings_yields_none() {
        let ctx = ToolContext::default(); // tool_mappings: None
        let input = serde_json::json!({ "path": "src/x.rs" });
        assert!(mapped_touch(&ctx, "cat_file", &input).is_none());
    }
}

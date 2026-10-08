//! Helpers for building `Attribution` values and dispatching
//! `FileTouchEvent`s to the `CoverageWriter` attached to a `ToolContext`.
//!
//! All helpers are no-ops when `ctx.services.coverage_writer` is `None`, so
//! there is no overhead in non-coverage runs beyond an `is_none()` check.

use crate::tool::ToolContext;
use rupu_coverage::{Attribution, FileTouchEvent};

/// The `Attribution` of a call made in the run `ctx` describes: its
/// identity, as the run assembler set it.
pub fn attribution_from(ctx: &ToolContext) -> Attribution {
    let id = &ctx.identity;
    let some = |s: &str| Some(s.to_string()).filter(|s| !s.is_empty());
    Attribution {
        run_id: id.run_id.clone(),
        model: id.model.clone(),
        surface: id.surface.coverage(),
        codename: id.codename.clone(),
        agent: some(&id.agent),
        provider: some(&id.provider),
    }
}

/// Emit a `FileTouchEvent` to the writer attached to `ctx`, if any.
/// Silently no-ops when `ctx.services.coverage_writer` is `None`.
pub async fn emit(ctx: &ToolContext, event: FileTouchEvent) {
    if let Some(writer) = &ctx.services.coverage_writer {
        writer.record_file_touch(event).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attribution_from_carries_agent_and_provider() {
        let ctx = ToolContext {
            identity: std::sync::Arc::new(crate::tool::RunIdentity {
                run_id: "r".into(),
                model: "m".into(),
                agent: "rev".into(),
                provider: "anthropic".into(),
                ..Default::default()
            }),
            ..ToolContext::default()
        };
        let a = attribution_from(&ctx);
        assert_eq!(a.agent.as_deref(), Some("rev"));
        assert_eq!(a.provider.as_deref(), Some("anthropic"));
        assert_eq!(a.model, "m");
    }
}

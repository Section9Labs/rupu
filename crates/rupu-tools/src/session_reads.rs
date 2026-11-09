//! Root-session local observations, never operator-MCP forwarding.
use crate::{
    descriptor::{Effect, Service, ToolDescriptor},
    output::{failed, ok_json},
    ports::SessionReadRequest,
    Tool, ToolContext, ToolError, ToolOutput,
};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

macro_rules! descriptor {
    ($constant:ident, $name:literal, $service:ident, $description:literal, $schema:ident) => {
        pub static $constant: ToolDescriptor = ToolDescriptor {
            name: $name,
            aliases: &[],
            effect: Effect::Read,
            needs: &[Service::$service],
            uses: &[],
            wildcard: true,
            description: $description,
            input_schema: $schema,
        };
    };
}
descriptor!(CONTEXT, "session.context", SessionContext, "Read this root session turn's frozen tools, skill names and owner. No credentials, prompts, other sessions, or permission changes. Selection is not proof of execution.", empty_schema);
descriptor!(WORKFLOWS, "session.workflows", SessionWorkflows, "Read bounded recorded workflow links belonging to this session only. Stored status is historical association evidence, not live workflow status. Partial inventory and omitted entries are explicit.", workflows_schema);
descriptor!(INSPECT, "run.inspect", RunInspection, "Inspect compact recorded event counts and child identities for this turn or a positively recorded descendant. ancestry is the ordered child run-ID chain from this turn; [] means this turn. Never reruns work or returns raw prompts/tool payloads. Reads are bounded and partial state is explicit.", ancestry_schema);
descriptor!(NETFLOWS, "run.netflows", RunNetflows, "Read bounded endpoint/outcome counts for this turn or a recorded descendant selected by ancestry. No URLs, headers, bodies, credentials or cross-project lookup. Captured prefix counts are not complete traffic coverage.", ancestry_schema);
fn empty_schema() -> Value {
    json!({"type":"object","properties":{},"required":[],"additionalProperties":false})
}
fn workflows_schema() -> Value {
    json!({"type":"object","properties":{"offset":{"type":"integer","minimum":0,"maximum":1000},"limit":{"type":"integer","minimum":1,"maximum":50}},"required":[],"additionalProperties":false})
}
fn ancestry_schema() -> Value {
    json!({"type":"object","properties":{"ancestry":{"type":"array","maxItems":8,"items":{"type":"string","maxLength":80}}},"required":[],"additionalProperties":false})
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Empty {}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Page {
    #[serde(default)]
    offset: usize,
    #[serde(default = "default_limit")]
    limit: usize,
}
fn default_limit() -> usize {
    25
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Chain {
    #[serde(default)]
    ancestry: Vec<String>,
}
pub struct SessionReadTool(pub &'static ToolDescriptor);
#[async_trait]
impl Tool for SessionReadTool {
    fn descriptor(&self) -> &'static ToolDescriptor {
        self.0
    }
    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let invalid = || ToolError::InvalidInput("invalid bounded session read arguments".into());
        let request = match self.0.name {
            "session.context" => {
                serde_json::from_value::<Empty>(input).map_err(|_| invalid())?;
                SessionReadRequest::Context
            }
            "session.workflows" => {
                let p: Page = serde_json::from_value(input).map_err(|_| invalid())?;
                if p.offset > 1000 || !(1..=50).contains(&p.limit) {
                    return Err(invalid());
                }
                SessionReadRequest::Workflows {
                    offset: p.offset,
                    limit: p.limit,
                }
            }
            "run.inspect" | "run.netflows" => {
                let c: Chain = serde_json::from_value(input).map_err(|_| invalid())?;
                if c.ancestry.len() > 8
                    || c.ancestry.iter().any(|s| {
                        s.is_empty()
                            || s.len() > 80
                            || !s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
                    })
                {
                    return Err(invalid());
                }
                if self.0.name == "run.inspect" {
                    SessionReadRequest::Inspect {
                        ancestry: c.ancestry,
                    }
                } else {
                    SessionReadRequest::Netflows {
                        ancestry: c.ancestry,
                    }
                }
            }
            _ => return Err(invalid()),
        };
        let service = ctx
            .services
            .session_reads
            .clone()
            .ok_or(ToolError::PermissionDenied)?;
        if !service.permits(self.0.name) {
            return Err(ToolError::PermissionDenied);
        }
        let ctx = ctx.clone();
        let result = tokio::task::spawn_blocking(move || service.read(request, &ctx))
            .await
            .map_err(|_| ToolError::Execution("session reader interrupted".into()))?;
        Ok(match result {
            Ok(value) => {
                let mut output = ok_json(value.clone());
                output.structured = Some(value);
                output
            }
            Err(error) => failed(error.to_string()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ports::{PortError, SessionReadPort},
        Service,
    };
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    struct Reader(Arc<AtomicUsize>);
    impl SessionReadPort for Reader {
        fn permits(&self, name: &str) -> bool {
            name == "session.context"
        }
        fn read(&self, request: SessionReadRequest, _: &ToolContext) -> Result<Value, PortError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            assert_eq!(request, SessionReadRequest::Context);
            Ok(json!({"session_id":"ses_fixture","run_id":"run_fixture"}))
        }
    }
    #[tokio::test]
    async fn service_requirement_and_strict_arguments_precede_reader() {
        let mut ctx = ToolContext::default();
        assert!(crate::bodies::body("session.context", &ctx).is_none());
        let calls = Arc::new(AtomicUsize::new(0));
        ctx.services.session_reads = Some(Arc::new(Reader(calls.clone())));
        assert!(ctx
            .services
            .provided(false)
            .contains(Service::SessionContext));
        let tool = SessionReadTool(&CONTEXT);
        assert!(tool
            .invoke(json!({"workspace_id":"ws_other"}), &ctx)
            .await
            .is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        let output = tool.invoke(json!({}), &ctx).await.unwrap();
        assert_eq!(output.structured.unwrap()["session_id"], "ses_fixture");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        for value in [
            json!({"limit":0}),
            json!({"offset":1001}),
            json!({"host":"remote"}),
        ] {
            assert!(SessionReadTool(&WORKFLOWS)
                .invoke(value, &ctx)
                .await
                .is_err());
        }
        assert!(SessionReadTool(&INSPECT)
            .invoke(json!({"ancestry":["../other"]}), &ctx)
            .await
            .is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
    #[test]
    fn legacy_wildcard_plus_one_exact_binding_cannot_offer_the_other_readers() {
        use crate::{grant::GrantInputs, AliasScope, ToolCatalog};
        let mut ctx = ToolContext::default();
        ctx.services.session_reads = Some(Arc::new(Reader(Arc::new(AtomicUsize::new(0)))));
        let available = ctx.services.provided(false);
        let declared = vec!["*".to_owned(), "session.context".to_owned()];
        let grant = ToolCatalog::builtin()
            .resolve_grant(GrantInputs {
                declared: Some(&declared),
                step_actions: &[],
                connector_cap: None,
                ambient: &[],
                available: &available,
                alias_scope: AliasScope::Everywhere,
            })
            .unwrap();
        assert!(grant.entries.contains_key("session.context"));
        for name in ["session.workflows", "run.inspect", "run.netflows"] {
            assert!(!grant.entries.contains_key(name));
            assert!(grant.skipped.iter().any(|entry| entry.tool == name));
            assert!(crate::bodies::body(name, &ctx).is_none());
        }
    }
}

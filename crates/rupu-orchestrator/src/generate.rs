//! Generate validated agent (`.md`) / workflow (`.yaml`) definition files
//! from a natural-language description via a configured provider/model.
//!
//! The core lives here (not in `rupu-runtime`) because validating a
//! generated workflow needs [`crate::Workflow::parse`], and orchestrator
//! already depends on `rupu-runtime` for `build_for_provider` — putting it
//! in runtime would cycle.

use rupu_providers::types::{LlmRequest, Message};
use rupu_runtime::provider_factory::build_for_provider;

/// Total send attempts (1 first try + repairs) before giving up.
pub const MAX_ATTEMPTS: u8 = 3;

/// Output token ceiling for a generated definition.
const MAX_TOKENS: u32 = 8192;

/// Provider preference order + the model each one generates with. Seeded
/// from each provider adapter's own `default_model()`. One place to bump.
pub const DEFAULT_GEN_MODELS: &[(&str, &str)] = &[
    ("anthropic", "claude-sonnet-4-6"),
    ("openai", "gpt-5.4"),
    ("gemini", "gemini-2.5-pro"),
    ("copilot", "claude-sonnet-4-6"),
];

/// What kind of definition to generate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenKind {
    Agent,
    Workflow,
}

impl GenKind {
    fn noun(self) -> &'static str {
        match self {
            GenKind::Agent => "agent",
            GenKind::Workflow => "workflow",
        }
    }
}

/// A request to generate one definition file.
#[derive(Debug, Clone)]
pub struct GenerateRequest {
    pub kind: GenKind,
    pub description: String,
    pub provider: String,
    pub model: String,
    /// Real agent names available in scope. Empty for agents; for
    /// workflows these are injected so generated steps reference agents
    /// that actually exist.
    pub available_agents: Vec<String>,
}

/// A successfully generated, validated definition.
#[derive(Debug, Clone)]
pub struct GenerateOutcome {
    pub content: String,
    pub provider: String,
    pub model: String,
    pub attempts: u8,
}

#[derive(thiserror::Error, Debug)]
pub enum GenerateError {
    #[error("no authenticated provider available; run `rupu auth login`")]
    NoCredentials,
    #[error("provider error: {0}")]
    Provider(#[from] rupu_providers::ProviderError),
    #[error("model returned empty output")]
    Empty,
    #[error("generated {kind} did not parse after {attempts} attempt(s): {last_error}")]
    Invalid {
        kind: &'static str,
        attempts: u8,
        last_error: String,
    },
}

/// Strip a single wrapping Markdown code fence (```` ```lang … ``` ````)
/// if the model wrapped its output, and trim surrounding whitespace.
pub fn strip_fences(raw: &str) -> &str {
    let t = raw.trim();
    if let Some(rest) = t.strip_prefix("```") {
        // Drop the (optional) language tag on the opening fence line.
        let after_lang = rest.split_once('\n').map(|(_, body)| body).unwrap_or("");
        if let Some(inner) = after_lang.trim_end().strip_suffix("```") {
            return inner.trim();
        }
    }
    t
}

/// Parse-validate generated content for the kind. Returns the parse error
/// text on failure (fed back into the repair prompt).
pub fn validate(kind: GenKind, content: &str) -> Result<(), String> {
    match kind {
        GenKind::Agent => rupu_agent::AgentSpec::parse(content)
            .map(|_| ())
            .map_err(|e| e.to_string()),
        GenKind::Workflow => crate::Workflow::parse(content)
            .map(|_| ())
            .map_err(|e| e.to_string()),
    }
}

/// Build the system prompt teaching the model the target file format.
pub fn build_system_prompt(kind: GenKind, available_agents: &[String]) -> String {
    match kind {
        GenKind::Agent => AGENT_SYSTEM_PROMPT.to_string(),
        GenKind::Workflow => {
            let agents = if available_agents.is_empty() {
                "(none defined yet \u{2014} you may reference an agent the user will create)"
                    .to_string()
            } else {
                available_agents.join(", ")
            };
            format!(
                "{WORKFLOW_SYSTEM_PROMPT}\n\nAvailable agent names (use ONLY these for `agent:` \
                 and panelist fields): {agents}"
            )
        }
    }
}

const AGENT_SYSTEM_PROMPT: &str = "You generate a rupu AGENT definition file. Output ONLY the \
file content \u{2014} no Markdown code fences, no commentary.\n\nFormat: YAML frontmatter \
delimited by `---` lines, then a Markdown body that is the agent's system prompt.\n\nRequired \
frontmatter:\n  name: <kebab-case identifier>\n  description: <one short line>\n  provider: \
anthropic   # one of: anthropic | openai | google | github-copilot | broker\n  model: <a model \
id for that provider, e.g. claude-sonnet-4-6>\n\nOptional frontmatter: tools (a YAML list, e.g. \
[bash, read, grep]), permissionMode (ask|bypass|readonly), maxTurns (integer).\n\nThe Markdown \
body after the closing `---` is the system prompt: role, voice, boundaries. Be specific and \
useful.\n";

const WORKFLOW_SYSTEM_PROMPT: &str = "You generate a rupu WORKFLOW definition file. Output ONLY \
the YAML content \u{2014} no Markdown code fences, no commentary.\n\nTop-level keys:\n  name: \
<kebab-case identifier>\n  description: <one short line>\n  inputs: (optional map) each input \
has type (string|int|bool), required (bool), description, optional default. Reference them in \
prompts as {{ inputs.<key> }}.\n  steps: (required list)\n\nEach linear step needs:\n  - id: \
<unique id>\n    agent: <one of the available agent names>\n    prompt: |\n      <multi-line \
instruction; may reference {{ inputs.x }} and {{ steps.<id>.output }}>\n    actions: []        \
# optional allow-list of tool actions\n\nOther step shapes: `parallel:` (a list of sub-steps \
each with id/agent/prompt), and `panel:` (panelists list + subject + prompt, optional gate). \
Keep it minimal unless the description calls for fan-out.\n\nNever leave `agent:` empty \
\u{2014} every linear/for_each step must name a real agent.\n";

/// First authenticated provider (in [`DEFAULT_GEN_MODELS`] order) paired
/// with its default generation model. `None` when nothing is authed.
pub async fn pick_default_gen_model(
    resolver: &dyn rupu_auth::CredentialResolver,
) -> Option<(String, String)> {
    for (provider, model) in DEFAULT_GEN_MODELS {
        if resolver.get(provider, None).await.is_ok() {
            return Some((provider.to_string(), model.to_string()));
        }
    }
    None
}

/// Generate a validated definition, repairing up to [`MAX_ATTEMPTS`].
pub async fn generate_definition(
    req: &GenerateRequest,
    resolver: &dyn rupu_auth::CredentialResolver,
) -> Result<GenerateOutcome, GenerateError> {
    let (_mode, mut provider) = build_for_provider(&req.provider, &req.model, None, resolver)
        .await
        .map_err(|_| GenerateError::NoCredentials)?;

    let system = build_system_prompt(req.kind, &req.available_agents);
    let mut messages = vec![Message::user(&format!(
        "Create a rupu {} from this description:\n\n{}",
        req.kind.noun(),
        req.description
    ))];

    let mut last_error = String::new();
    for attempt in 1..=MAX_ATTEMPTS {
        let llm_req = LlmRequest {
            model: req.model.clone(),
            system: Some(system.clone()),
            messages: messages.clone(),
            max_tokens: MAX_TOKENS,
            ..Default::default()
        };
        let resp = provider.send(&llm_req).await?;
        let raw = resp.text().ok_or(GenerateError::Empty)?.to_string();
        let content = strip_fences(&raw).to_string();

        match validate(req.kind, &content) {
            Ok(()) => {
                return Ok(GenerateOutcome {
                    content,
                    provider: req.provider.clone(),
                    model: req.model.clone(),
                    attempts: attempt,
                });
            }
            Err(e) => {
                last_error = e;
                messages.push(Message::assistant(&raw));
                messages.push(Message::user(&format!(
                    "Your previous output failed to parse as a valid rupu {}: {last_error}\n\nReturn the corrected full file. Output ONLY the file content.",
                    req.kind.noun()
                )));
            }
        }
    }

    Err(GenerateError::Invalid {
        kind: req.kind.noun(),
        attempts: MAX_ATTEMPTS,
        last_error,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_fences_removes_lang_fence() {
        let input = "```yaml\nname: hi\n```";
        assert_eq!(strip_fences(input), "name: hi");
    }

    #[test]
    fn strip_fences_leaves_bare_content() {
        assert_eq!(strip_fences("name: hi"), "name: hi");
    }

    #[test]
    fn validate_accepts_good_workflow() {
        let yaml = "name: wf\nsteps:\n  - id: main\n    agent: rev\n    prompt: do it\n";
        assert!(validate(GenKind::Workflow, yaml).is_ok());
    }

    #[test]
    fn validate_rejects_workflow_missing_agent() {
        let yaml = "name: wf\nsteps:\n  - id: main\n    prompt: do it\n";
        let err = validate(GenKind::Workflow, yaml).unwrap_err();
        assert!(err.contains("agent"), "got: {err}");
    }

    #[test]
    fn system_prompt_lists_available_agents_for_workflows() {
        let p = build_system_prompt(
            GenKind::Workflow,
            &["reviewer".to_string(), "fixer".to_string()],
        );
        assert!(p.contains("reviewer"));
        assert!(p.contains("fixer"));
    }

    use rupu_auth::backend::ProviderId;
    use rupu_auth::in_memory::InMemoryResolver;
    use rupu_auth::stored::StoredCredential;
    use rupu_providers::AuthMode;
    use tokio::sync::Mutex as AsyncMutex;

    // Env-var seam is process-global; serialize.
    static ENV_LOCK: AsyncMutex<()> = AsyncMutex::const_new(());

    const VALID_AGENT_MD: &str = "---\nname: gen-agent\ndescription: a test agent\nprovider: anthropic\nmodel: claude-sonnet-4-6\n---\n\nYou are a helpful test agent.\n";

    #[tokio::test]
    async fn generate_returns_valid_content_first_try() {
        let _g = ENV_LOCK.lock().await;
        let script = serde_json::json!([
            { "AssistantText": { "text": VALID_AGENT_MD, "stop": "end_turn" } }
        ])
        .to_string();
        std::env::set_var("RUPU_MOCK_PROVIDER_SCRIPT", &script);

        let resolver = InMemoryResolver::new();
        let req = GenerateRequest {
            kind: GenKind::Agent,
            description: "a helpful test agent".into(),
            provider: "anthropic".into(),
            model: "claude-sonnet-4-6".into(),
            available_agents: vec![],
        };
        let out = generate_definition(&req, &resolver).await;
        std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
        let out = out.expect("ok");

        assert_eq!(out.attempts, 1);
        assert!(out.content.contains("name: gen-agent"));
    }

    #[tokio::test]
    async fn generate_repairs_invalid_then_succeeds() {
        let _g = ENV_LOCK.lock().await;
        // First turn: invalid (no frontmatter). Second turn: valid.
        let script = serde_json::json!([
            { "AssistantText": { "text": "not a valid agent file", "stop": "end_turn" } },
            { "AssistantText": { "text": VALID_AGENT_MD, "stop": "end_turn" } }
        ])
        .to_string();
        std::env::set_var("RUPU_MOCK_PROVIDER_SCRIPT", &script);

        let resolver = InMemoryResolver::new();
        let req = GenerateRequest {
            kind: GenKind::Agent,
            description: "x".into(),
            provider: "anthropic".into(),
            model: "claude-sonnet-4-6".into(),
            available_agents: vec![],
        };
        let out = generate_definition(&req, &resolver).await;
        std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
        let out = out.expect("ok");

        assert_eq!(out.attempts, 2);
        assert!(out.content.contains("name: gen-agent"));
    }

    #[tokio::test]
    async fn generate_errors_when_never_valid() {
        let _g = ENV_LOCK.lock().await;
        let script = serde_json::json!([
            { "AssistantText": { "text": "junk", "stop": "end_turn" } },
            { "AssistantText": { "text": "still junk", "stop": "end_turn" } },
            { "AssistantText": { "text": "junk again", "stop": "end_turn" } }
        ])
        .to_string();
        std::env::set_var("RUPU_MOCK_PROVIDER_SCRIPT", &script);

        let resolver = InMemoryResolver::new();
        let req = GenerateRequest {
            kind: GenKind::Agent,
            description: "x".into(),
            provider: "anthropic".into(),
            model: "claude-sonnet-4-6".into(),
            available_agents: vec![],
        };
        let out = generate_definition(&req, &resolver).await;
        std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
        let err = out.unwrap_err();

        match err {
            GenerateError::Invalid { attempts, .. } => assert_eq!(attempts, MAX_ATTEMPTS),
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn pick_default_returns_none_when_nothing_authed() {
        let resolver = InMemoryResolver::new();
        assert!(pick_default_gen_model(&resolver).await.is_none());
    }

    #[tokio::test]
    async fn pick_default_prefers_anthropic_then_openai() {
        // Only openai authed → openai wins.
        let resolver = InMemoryResolver::new();
        resolver
            .put(
                ProviderId::Openai,
                AuthMode::ApiKey,
                StoredCredential::api_key("sk-test-openai"),
            )
            .await;
        let (provider, model) = pick_default_gen_model(&resolver).await.expect("some");
        assert_eq!(provider, "openai");
        assert_eq!(model, "gpt-5.4");

        // Add anthropic → anthropic now wins (higher preference).
        resolver
            .put(
                ProviderId::Anthropic,
                AuthMode::ApiKey,
                StoredCredential::api_key("sk-test-anthropic"),
            )
            .await;
        let (provider, model) = pick_default_gen_model(&resolver).await.expect("some");
        assert_eq!(provider, "anthropic");
        assert_eq!(model, "claude-sonnet-4-6");
    }
}

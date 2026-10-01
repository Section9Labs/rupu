use rupu_agent::spec::AgentSpec;
use rupu_providers::AuthMode;

const WITH_AUTH: &str = "---
name: test
provider: anthropic
auth: sso
model: claude-sonnet-4-6
---
You are a test agent.";

const WITHOUT_AUTH: &str = "---
name: test
provider: anthropic
model: claude-sonnet-4-6
---
You are a test agent.";

const WITH_API_KEY_AUTH: &str = "---
name: test
provider: openai
auth: api-key
model: gpt-5
---
hi";

#[test]
fn parses_explicit_sso_auth() {
    let spec = AgentSpec::parse(WITH_AUTH).unwrap();
    assert_eq!(spec.auth, Some(AuthMode::Sso));
    assert_eq!(spec.provider.as_deref(), Some("anthropic"));
}

#[test]
fn parses_explicit_api_key_auth() {
    let spec = AgentSpec::parse(WITH_API_KEY_AUTH).unwrap();
    assert_eq!(spec.auth, Some(AuthMode::ApiKey));
}

#[test]
fn auth_field_optional_for_backwards_compat() {
    let spec = AgentSpec::parse(WITHOUT_AUTH).unwrap();
    assert_eq!(spec.auth, None);
    assert_eq!(spec.provider.as_deref(), Some("anthropic"));
}

#[test]
fn unknown_auth_value_is_a_parse_error() {
    let bad = "---
name: test
provider: anthropic
auth: bogus
model: claude-sonnet-4-6
---
hi";
    assert!(AgentSpec::parse(bad).is_err());
}

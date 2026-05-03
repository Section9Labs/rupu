use rupu_config::{Config, ProviderConfig};

#[test]
fn provider_config_parses_with_overrides() {
    let toml = r#"
[providers.anthropic]
base_url = "https://example-proxy.test"
timeout_ms = 60000
max_retries = 5
max_concurrency = 4
default_model = "claude-sonnet-4-6"

[providers.openai]
org_id = "org-abc123"
"#;
    let cfg: Config = toml::from_str(toml).expect("parse");
    let anthro = cfg.providers.get("anthropic").expect("anthropic block");
    assert_eq!(
        anthro.base_url.as_deref(),
        Some("https://example-proxy.test")
    );
    assert_eq!(anthro.timeout_ms, Some(60000));
    assert_eq!(anthro.max_retries, Some(5));
    assert_eq!(anthro.max_concurrency, Some(4));
    assert_eq!(anthro.default_model.as_deref(), Some("claude-sonnet-4-6"));

    let openai = cfg.providers.get("openai").expect("openai block");
    assert_eq!(openai.org_id.as_deref(), Some("org-abc123"));
    assert_eq!(openai.base_url, None);

    // Unset block: default_model is None
    assert!(!cfg.providers.contains_key("gemini"));
}

#[test]
fn provider_config_empty_when_unset() {
    let cfg: Config = toml::from_str("").expect("parse empty");
    assert!(cfg.providers.is_empty());
}

#[test]
fn provider_config_serialize_omits_none_fields() {
    let mut cfg = Config::default();
    cfg.providers.insert(
        "anthropic".into(),
        ProviderConfig {
            base_url: Some("https://x.test".into()),
            ..Default::default()
        },
    );
    let s = toml::to_string(&cfg).unwrap();
    assert!(s.contains("[providers.anthropic]"));
    assert!(s.contains("base_url = \"https://x.test\""));
    assert!(!s.contains("timeout_ms"));
}

use rupu_config::{Config, ScmPlatformConfig};

#[test]
fn scm_default_parses_with_owner_and_repo() {
    let toml = r#"
[scm.default]
platform = "github"
owner = "section9labs"
repo = "rupu"

[issues.default]
tracker = "github"
project = "section9labs/rupu"
"#;
    let cfg: Config = toml::from_str(toml).expect("parse");
    let scm = cfg.scm.default.as_ref().expect("scm.default present");
    assert_eq!(scm.platform.as_deref(), Some("github"));
    assert_eq!(scm.owner.as_deref(), Some("section9labs"));
    assert_eq!(scm.repo.as_deref(), Some("rupu"));

    let iss = cfg.issues.default.as_ref().expect("issues.default present");
    assert_eq!(iss.tracker.as_deref(), Some("github"));
    assert_eq!(iss.project.as_deref(), Some("section9labs/rupu"));
}

#[test]
fn scm_platform_config_parses_per_platform_overrides() {
    let toml = r#"
[scm.github]
base_url = "https://ghe.example.com/api/v3"
timeout_ms = 30000
max_concurrency = 8
clone_protocol = "https"

[scm.gitlab]
base_url = "https://gitlab.example.com/api/v4"
clone_protocol = "ssh"
"#;
    let cfg: Config = toml::from_str(toml).expect("parse");
    let gh = cfg.scm.platforms.get("github").expect("github platform");
    assert_eq!(
        gh.base_url.as_deref(),
        Some("https://ghe.example.com/api/v3")
    );
    assert_eq!(gh.timeout_ms, Some(30000));
    assert_eq!(gh.max_concurrency, Some(8));
    assert_eq!(gh.clone_protocol.as_deref(), Some("https"));

    let gl = cfg.scm.platforms.get("gitlab").expect("gitlab platform");
    assert_eq!(gl.clone_protocol.as_deref(), Some("ssh"));
}

#[test]
fn empty_scm_section_yields_default() {
    let cfg: Config = toml::from_str("").expect("parse empty");
    assert!(cfg.scm.default.is_none());
    assert!(cfg.scm.platforms.is_empty());
    assert!(cfg.issues.default.is_none());
}

#[test]
fn scm_platform_config_serialize_omits_none() {
    let p = ScmPlatformConfig {
        base_url: Some("https://x.test".into()),
        ..Default::default()
    };
    let s = toml::to_string(&p).unwrap();
    assert!(s.contains("base_url = \"https://x.test\""));
    assert!(!s.contains("timeout_ms"));
}

/// A GitLab account's own OAuth application (a self-managed instance's, or
/// one registered on gitlab.com in place of glab's default).
#[test]
fn scm_platform_config_parses_an_oauth_client_id() {
    let toml = r#"
[scm.gl-corp]
kind = "gitlab"
base_url = "https://gitlab.example.com/api/v4"
oauth_client_id = "corp-app"
"#;
    let cfg: Config = toml::from_str(toml).expect("parse");
    let gl = cfg.scm.platforms.get("gl-corp").expect("gl-corp account");
    assert_eq!(gl.oauth_client_id.as_deref(), Some("corp-app"));
    assert_eq!(
        cfg.scm.platforms.get("gl-corp"),
        Some(&ScmPlatformConfig {
            kind: Some("gitlab".into()),
            base_url: Some("https://gitlab.example.com/api/v4".into()),
            oauth_client_id: Some("corp-app".into()),
            ..Default::default()
        })
    );
}

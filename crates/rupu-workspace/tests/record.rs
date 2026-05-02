use rupu_workspace::Workspace;

#[test]
fn round_trip_workspace_toml() {
    let ws = Workspace {
        id: "ws_01HXXX0123456789ABCDEFGHJK".into(),
        path: "/Users/matt/Code/rupu".into(),
        repo_remote: Some("git@github.com:section9labs/rupu.git".into()),
        initial_branch: Some("main".into()),
        created_at: "2026-05-01T17:00:00Z".into(),
        last_run_at: Some("2026-05-01T17:42:00Z".into()),
    };
    let serialized = toml::to_string(&ws).unwrap();
    let back: Workspace = toml::from_str(&serialized).unwrap();
    assert_eq!(ws, back);
}

#[test]
fn parses_minimal_workspace_toml() {
    let toml = r#"
id              = "ws_01HXXX0123456789ABCDEFGHJK"
path            = "/Users/matt/Code/rupu"
created_at      = "2026-05-01T17:00:00Z"
"#;
    let ws: Workspace = toml::from_str(toml).unwrap();
    assert_eq!(ws.repo_remote, None);
    assert_eq!(ws.initial_branch, None);
}

use rupu_agent::resolve_mode;
use rupu_tools::PermissionMode;

#[test]
fn cli_flag_wins_over_everything() {
    let m = resolve_mode(
        Some(PermissionMode::Bypass),
        Some(PermissionMode::Ask),
        Some(PermissionMode::Readonly),
        Some(PermissionMode::Ask),
    );
    assert_eq!(m, PermissionMode::Bypass);
}

#[test]
fn agent_frontmatter_wins_over_config() {
    let m = resolve_mode(
        None,
        Some(PermissionMode::Readonly),
        Some(PermissionMode::Bypass),
        Some(PermissionMode::Ask),
    );
    assert_eq!(m, PermissionMode::Readonly);
}

#[test]
fn project_wins_over_global_config() {
    let m = resolve_mode(
        None,
        None,
        Some(PermissionMode::Bypass),
        Some(PermissionMode::Readonly),
    );
    assert_eq!(m, PermissionMode::Bypass);
}

#[test]
fn global_config_used_when_nothing_else_set() {
    let m = resolve_mode(None, None, None, Some(PermissionMode::Bypass));
    assert_eq!(m, PermissionMode::Bypass);
}

#[test]
fn default_is_ask_when_all_unset() {
    let m = resolve_mode(None, None, None, None);
    assert_eq!(m, PermissionMode::Ask);
}

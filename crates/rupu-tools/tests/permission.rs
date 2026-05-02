use rupu_tools::{PermissionGate, PermissionMode};

#[test]
fn readonly_denies_writers() {
    let gate = PermissionGate::for_mode(PermissionMode::Readonly);
    assert!(!gate.allow_unconditionally("bash"));
    assert!(!gate.allow_unconditionally("write_file"));
    assert!(!gate.allow_unconditionally("edit_file"));
    assert!(gate.allow_unconditionally("read_file"));
    assert!(gate.allow_unconditionally("grep"));
    assert!(gate.allow_unconditionally("glob"));
}

#[test]
fn bypass_allows_everything() {
    let gate = PermissionGate::for_mode(PermissionMode::Bypass);
    for tool in [
        "bash",
        "write_file",
        "edit_file",
        "read_file",
        "grep",
        "glob",
    ] {
        assert!(
            gate.allow_unconditionally(tool),
            "{tool} denied under bypass"
        );
    }
}

#[test]
fn ask_allows_readers_unconditionally() {
    let gate = PermissionGate::for_mode(PermissionMode::Ask);
    assert!(gate.allow_unconditionally("read_file"));
    assert!(gate.allow_unconditionally("grep"));
    assert!(gate.allow_unconditionally("glob"));
}

#[test]
fn ask_requires_decision_for_writers() {
    let gate = PermissionGate::for_mode(PermissionMode::Ask);
    assert!(!gate.allow_unconditionally("bash"));
    assert!(gate.requires_decision("bash"));
    assert!(gate.requires_decision("write_file"));
    assert!(gate.requires_decision("edit_file"));
    assert!(!gate.requires_decision("read_file"));
}

#[test]
fn unknown_tool_is_denied() {
    let gate = PermissionGate::for_mode(PermissionMode::Bypass);
    // Even bypass shouldn't whitelist a tool we don't know about.
    // The runtime would refuse to dispatch it, but the gate also says no.
    assert!(!gate.allow_unconditionally("unknown_tool"));
}

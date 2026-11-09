use super::*;
use std::fs;

fn fixture() -> (tempfile::TempDir, LayerDirs) {
    let tmp = tempfile::tempdir().unwrap();
    let dirs = LayerDirs {
        global: Some(tmp.path().join("global")),
        project: Some(tmp.path().join("project")),
        overlay: vec![],
    };
    (tmp, dirs)
}
fn install(dirs: &LayerDirs, layer: Layer, name: &str, extra: &str) {
    let dir = dirs.skills_dir(layer).unwrap().join(name);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: Fixture guidance.\n{extra}---\nFixture body.\n"),
    )
    .unwrap();
}
fn full(dirs: &LayerDirs) -> SkillPlan {
    plan_for(dirs, Some(FULL_PRESET_ID), &[], &[]).unwrap()
}

#[test]
fn pinned_inventory_has_exact_110_unique_qualified_entries() {
    assert_eq!(FULL_PRESET_ENTRIES.len(), 110);
    let inventory: serde_json::Value = serde_json::from_str(include_str!(
        "../../../docs/session-full-skill-preset-inventory.json"
    ))
    .unwrap();
    assert_eq!(inventory["commit"], FULL_PRESET_COMMIT);
    let mut qualified: Vec<String> = inventory["skills"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| format!("global:{}", r["name"].as_str().unwrap()))
        .collect();
    qualified.extend(FULL_PRESET_LOCAL_ENTRIES.iter().map(|s| (*s).to_owned()));
    qualified.sort();
    assert_eq!(
        qualified,
        FULL_PRESET_ENTRIES
            .iter()
            .map(|s| (*s).to_owned())
            .collect::<Vec<_>>()
    );
    assert!(FULL_PRESET_ENTRIES.windows(2).all(|w| w[0] < w[1]));
    assert!(FULL_PRESET_ENTRIES.iter().all(|e| matches!(
        e.parse::<rupu_skills::SkillGrant>(),
        Ok(rupu_skills::SkillGrant::Qualified(..))
    )));
    assert!(PRESET_ENTRIES
        .iter()
        .all(|e| FULL_PRESET_ENTRIES.contains(e)));
    assert_eq!(
        FULL_PRESET_COMMIT,
        "d57aa11a8964202ab28d1c21818c97dcedc0d6f1"
    );
}
#[test]
fn full_only_indexes_installed_inventory_and_preloads_operations() {
    let (_tmp, dirs) = fixture();
    for name in ["code-review", "rupu-session-operations", "unrelated"] {
        install(&dirs, Layer::Global, name, "");
    }
    let p = full(&dirs);
    assert_eq!(p.preset.id, FULL_PRESET_ID);
    assert_eq!(p.selected.len(), 2);
    assert_eq!(p.diagnostics.len(), 108);
    let code = p.selected.iter().find(|s| s.name == "code-review").unwrap();
    assert!(code.indexed && !code.preload);
    let ops = p
        .selected
        .iter()
        .find(|s| s.name == "rupu-session-operations")
        .unwrap();
    assert!(!ops.indexed && ops.preload && ops.content_hash.is_some());
    let inventory = p.inventory.unwrap();
    assert_eq!(inventory.expected_entries.len(), 110);
    assert_eq!(
        inventory.local_entries,
        vec!["global:rupu-session-operations"]
    );
}
#[test]
fn full_missing_inventory_never_installs_or_creates_directories() {
    let (tmp, dirs) = fixture();
    let p = full(&dirs);
    assert!(p.selected.is_empty());
    assert_eq!(p.diagnostics.len(), 110);
    assert!(p
        .diagnostics
        .iter()
        .all(|d| d.code == SkillDiagnosticCode::NotInstalled));
    assert_eq!(fs::read_dir(tmp.path()).unwrap().count(), 0);
}
#[test]
fn legacy_default_and_opt_out_do_not_expand() {
    let (_tmp, dirs) = fixture();
    install(&dirs, Layer::Global, "code-review", "");
    install(&dirs, Layer::Global, "work-units", "");
    for preset in [None, Some(PRESET_ID)] {
        let p = plan_for(&dirs, preset, &[], &[]).unwrap();
        assert_eq!(p.selected.len(), 1);
        assert_eq!(p.selected[0].name, "work-units");
        assert_eq!(p.preset.id, PRESET_ID);
        assert!(p.inventory.is_none());
    }
    assert!(plan_for(&dirs, Some("none"), &[], &[])
        .unwrap()
        .selected
        .is_empty());
    assert!(plan_for(&dirs, Some("unknown"), &[], &[]).is_err());
}
#[test]
fn full_exact_global_inventory_never_guesses_project_layer() {
    let (_tmp, dirs) = fixture();
    install(&dirs, Layer::Project, "code-review", "");
    assert!(full(&dirs).selected.is_empty());
    install(&dirs, Layer::Global, "code-review", "");
    assert_eq!(full(&dirs).selected[0].layer, Layer::Global);
    let explicit = vec!["project:code-review".into()];
    let p = plan_for(&dirs, Some(FULL_PRESET_ID), &explicit, &[]).unwrap();
    assert_eq!(p.selected.len(), 1);
    assert_eq!(p.selected[0].layer, Layer::Project);
    assert!(plan_for(&dirs, Some(FULL_PRESET_ID), &["code-review".into()], &[]).is_err());
}
#[test]
fn full_invalid_and_explicit_only_entries_are_diagnostic_not_indexed() {
    let (_tmp, dirs) = fixture();
    install(
        &dirs,
        Layer::Global,
        "code-review",
        "disable-model-invocation: true\n",
    );
    install(&dirs, Layer::Global, "recon", "");
    fs::write(
        dirs.global.as_ref().unwrap().join("recon/SKILL.md"),
        "invalid",
    )
    .unwrap();
    let p = full(&dirs);
    assert!(p.selected.is_empty());
    assert!(p
        .diagnostics
        .iter()
        .any(|d| d.entry.as_deref() == Some("global:code-review")
            && d.code == SkillDiagnosticCode::NotModelLoadable));
    assert!(p
        .diagnostics
        .iter()
        .any(|d| d.entry.as_deref() == Some("global:recon")
            && d.code == SkillDiagnosticCode::Invalid));
    let p = plan_for(
        &dirs,
        Some(FULL_PRESET_ID),
        &[],
        &["global:code-review".into()],
    )
    .unwrap();
    assert!(p.selected[0].preload);
}
#[test]
fn frozen_full_selection_never_grows_after_install() {
    let (_tmp, dirs) = fixture();
    install(&dirs, Layer::Global, "code-review", "");
    let p = full(&dirs);
    install(&dirs, Layer::Global, "recon", "");
    let status = frozen(&dirs, &p.selected);
    assert_eq!(status.entries.len(), 1);
    assert_eq!(status.entries[0].entry, "global:code-review");
    assert!(status.ready);
    assert_eq!(full(&dirs).selected.len(), 2);
}

#[test]
fn complete_pinned_suite_freezes_110_instructions_without_unrelated_entries() {
    let (_tmp, dirs) = fixture();
    for entry in FULL_PRESET_ENTRIES {
        install(
            &dirs,
            Layer::Global,
            entry.strip_prefix("global:").unwrap(),
            "",
        );
    }
    install(&dirs, Layer::Global, "unrelated", "");
    let p = full(&dirs);
    assert_eq!(p.selected.len(), 110);
    assert_eq!(p.selected.iter().filter(|s| s.indexed).count(), 109);
    assert_eq!(p.selected.iter().filter(|s| s.preload).count(), 1);
    assert!(p.diagnostics.is_empty());
    assert_eq!(
        serde_json::from_str::<SkillPlan>(&serde_json::to_string(&p).unwrap()).unwrap(),
        p
    );
}

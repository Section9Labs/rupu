//! Read-only planning and inspection of a session's curated skill selections.
//! The preset names exact installed skills; it installs nothing, grants no
//! tools, and never expands a frozen session when the catalog changes.

use rupu_skills::{
    resolve_frozen, resolve_run_skills, DiscoverError, FrozenSkill, Layer, LayerDirs,
    MissingReason, RunSkillsError, SkillCatalog, SkillReason, SkillRequest,
};
use serde::{Deserialize, Serialize};

pub const PRESET_ID: &str = "rupu-session-v1";
pub const PRESET_VERSION: u32 = 1;
pub const PRESET_ENTRIES: &[&str] = &[
    "global:coverage-assets",
    "global:finding-report",
    "global:rupu-session-operations",
    "global:work-units",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresetMetadata {
    pub id: String,
    pub version: u32,
    pub enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillDiagnosticCode {
    NotInstalled,
    Invalid,
    NotModelLoadable,
    UnreadableLayer,
    Removed,
    Changed,
    InvalidFrozenRecord,
    Conflict,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillDiagnostic {
    /// None for an error affecting discovery of an entire layer.
    pub entry: Option<String>,
    pub layer: Layer,
    pub code: SkillDiagnosticCode,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillPlan {
    pub preset: PresetMetadata,
    /// The actual combined explicit, preload, and preset selections.
    pub selected: Vec<FrozenSkill>,
    /// All qualified preset grant additions; these never contain a wildcard.
    pub defaults: Vec<String>,
    /// Default additions whose bodies should accompany the first turn.
    /// Only the operations instructions are preloaded; the others stay indexed.
    #[serde(default)]
    pub preload_defaults: Vec<String>,
    pub diagnostics: Vec<SkillDiagnostic>,
}

impl SkillPlan {
    /// Add these defaults to the original explicit grants before the existing
    /// session freeze. Also merge `preload_defaults` with the original preload
    /// grants so operations instructions are present without a model load.
    pub fn selection(&self) -> Vec<String> {
        self.defaults.clone()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SkillPlanError {
    #[error("cannot discover {layer} skills: {source}")]
    Discovery {
        layer: Layer,
        #[source]
        source: DiscoverError,
    },
    #[error(transparent)]
    Resolve(#[from] RunSkillsError),
}

/// Resolve a proposed selection without writing state or launching work.
/// Explicit requests use the existing CLI grammar, including its wildcards;
/// callers exposing a narrower API must reject wildcards at that boundary.
pub fn plan(
    dirs: &LayerDirs,
    enabled: bool,
    explicit: &[String],
    preload: &[String],
) -> Result<SkillPlan, SkillPlanError> {
    let mut out = SkillPlan {
        preset: PresetMetadata {
            id: PRESET_ID.into(),
            version: PRESET_VERSION,
            enabled,
        },
        selected: Vec::new(),
        defaults: Vec::new(),
        preload_defaults: Vec::new(),
        diagnostics: Vec::new(),
    };
    // Match the existing no-skills path: an opted-out, empty request need
    // not discover either layer, including an unrelated unreadable layer.
    if !enabled && explicit.is_empty() && preload.is_empty() {
        return Ok(out);
    }
    let (catalog, failures) = SkillCatalog::discover_readable(dirs);
    if let Some((layer, source)) = failures.into_iter().next() {
        return Err(SkillPlanError::Discovery { layer, source });
    }
    let request = SkillRequest {
        reason: SkillReason::Session,
        grants: explicit,
        preload,
    };
    // Resolve explicit requests first, preserving ambiguous, invalid and
    // both-layer errors before optional defaults are considered.
    let requested = resolve_run_skills(&catalog, &[request])?;
    if enabled {
        for &qualified in PRESET_ENTRIES {
            let name = qualified.strip_prefix("global:").expect("global preset");
            if requested.get(name).is_some() {
                continue;
            }
            let diagnostic = |code, message| SkillDiagnostic {
                entry: Some(qualified.into()),
                layer: Layer::Global,
                code,
                message,
            };
            match catalog.get(Layer::Global, name) {
                None => out.diagnostics.push(diagnostic(
                    SkillDiagnosticCode::NotInstalled,
                    format!("optional preset skill {qualified} is not installed"),
                )),
                Some(entry) => match entry.skill() {
                    Err(error) => out.diagnostics.push(diagnostic(
                        SkillDiagnosticCode::Invalid,
                        format!("optional preset skill {qualified} is invalid: {error}"),
                    )),
                    Ok(skill) if skill.frontmatter.disable_model_invocation => {
                        out.diagnostics.push(diagnostic(
                            SkillDiagnosticCode::NotModelLoadable,
                            format!("optional preset skill {qualified} disables model invocation"),
                        ));
                    }
                    Ok(skill) => {
                        if qualified == "global:rupu-session-operations" {
                            if let Err(error) = skill.content_hash() {
                                out.diagnostics.push(diagnostic(
                                    SkillDiagnosticCode::Invalid,
                                    format!("optional preset skill {qualified} cannot be preloaded: {error}"),
                                ));
                                continue;
                            }
                            out.preload_defaults.push(qualified.into());
                        }
                        out.defaults.push(qualified.into());
                    }
                },
            }
        }
    }
    out.selected = resolve_run_skills(
        &catalog,
        &[
            SkillRequest {
                reason: SkillReason::Session,
                grants: explicit,
                preload,
            },
            SkillRequest {
                reason: SkillReason::Session,
                grants: &out.defaults,
                preload: &out.preload_defaults,
            },
        ],
    )?
    .freeze();
    Ok(out)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrozenSkillStatus {
    pub entry: String,
    pub frozen: FrozenSkill,
    /// None means the layer could not be read, not that the skill is absent.
    pub installed: Option<bool>,
    /// None when absent or discovery failed; Some(false) for an invalid skill.
    pub valid: Option<bool>,
    pub model_loadable: bool,
    pub current_stamp: Option<String>,
    pub changed: Option<bool>,
    pub missing: Option<SkillDiagnosticCode>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrozenSkillReadiness {
    /// Every frozen entry can currently be resolved. Changes alone do not block.
    pub ready: bool,
    pub entries: Vec<FrozenSkillStatus>,
    pub diagnostics: Vec<SkillDiagnostic>,
}

/// Inspect the stored selection exactly as frozen, without adding newly
/// installed preset members or changing flags and baseline versions.
pub fn frozen(dirs: &LayerDirs, selected: &[FrozenSkill]) -> FrozenSkillReadiness {
    let mut out = FrozenSkillReadiness {
        ready: true,
        entries: Vec::new(),
        diagnostics: Vec::new(),
    };
    if selected.is_empty() {
        return out;
    }
    let (catalog, failures) = SkillCatalog::discover_readable(dirs);
    for (layer, error) in &failures {
        out.ready = false;
        out.diagnostics.push(SkillDiagnostic {
            entry: None,
            layer: *layer,
            code: SkillDiagnosticCode::UnreadableLayer,
            message: error.to_string(),
        });
    }
    let (resolved, missing) = resolve_frozen(&catalog, selected);
    for record in selected {
        let entry = format!("{}:{}", record.layer, record.name);
        let mut status = FrozenSkillStatus {
            entry: entry.clone(),
            frozen: record.clone(),
            installed: None,
            valid: None,
            model_loadable: false,
            current_stamp: None,
            changed: None,
            missing: None,
        };
        if failures.iter().any(|(layer, _)| *layer == record.layer) {
            status.missing = Some(SkillDiagnosticCode::UnreadableLayer);
            out.entries.push(status);
            continue;
        }
        let installed = catalog.get(record.layer, &record.name);
        status.installed = Some(installed.is_some());
        if let Some(installed) = installed {
            status.valid = Some(installed.skill().is_ok());
            if let Ok(skill) = installed.skill() {
                status.current_stamp = Some(skill.stamp.hex());
                status.changed = rupu_skills::Stamp::from_hex(&record.stamp)
                    .map(|baseline| baseline != skill.stamp);
            }
        }
        if let Some(missing) = missing
            .iter()
            .find(|missing| missing.name == record.name && missing.layer == record.layer)
        {
            let code = match &missing.reason {
                MissingReason::Removed => SkillDiagnosticCode::Removed,
                MissingReason::Invalid(_) => SkillDiagnosticCode::Invalid,
                MissingReason::Conflict { .. } => SkillDiagnosticCode::Conflict,
                MissingReason::BadStamp
                | MissingReason::BadHash
                | MissingReason::PreloadHashMismatch => SkillDiagnosticCode::InvalidFrozenRecord,
            };
            status.missing = Some(code);
            out.ready = false;
            out.diagnostics.push(SkillDiagnostic {
                entry: Some(entry.clone()),
                layer: record.layer,
                code,
                message: missing.reason.to_string(),
            });
        } else {
            status.model_loadable = resolved
                .get(&record.name)
                .filter(|skill| skill.skill.layer == record.layer)
                .is_some_and(|skill| skill.model_loadable());
        }
        if status.changed == Some(true) {
            out.diagnostics.push(SkillDiagnostic {
                entry: Some(entry),
                layer: record.layer,
                code: SkillDiagnosticCode::Changed,
                message: "installed skill differs from the session's frozen baseline".into(),
            });
        }
        out.entries.push(status);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, path::Path};

    fn install(root: &Path, name: &str, extra: &str) {
        let dir = root.join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: Fixture skill.\n{extra}---\nFixture instructions.\n"),
        )
        .unwrap();
    }

    fn dirs(root: &Path) -> LayerDirs {
        LayerDirs {
            global: Some(root.join("global/skills")),
            project: Some(root.join("project/.rupu/skills")),
            overlay: Vec::new(),
        }
    }

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).into()).collect()
    }

    #[test]
    fn preset_selects_exact_members_and_preloads_only_operations() {
        let temp = tempfile::tempdir().unwrap();
        let dirs = dirs(temp.path());
        let global = dirs.global.as_deref().unwrap();
        for name in [
            "coverage-assets",
            "finding-report",
            "rupu-session-operations",
            "work-units",
            "unrelated",
        ] {
            install(global, name, "");
        }
        install(dirs.project.as_deref().unwrap(), "coverage-assets", "");
        let plan = plan(&dirs, true, &[], &[]).unwrap();
        assert_eq!(plan.selection(), strings(PRESET_ENTRIES));
        assert_eq!(plan.selected.len(), 4);
        assert!(plan.selection().iter().all(|entry| !entry.contains('*')));
        assert_eq!(
            plan.preload_defaults,
            strings(&["global:rupu-session-operations"])
        );
        assert!(plan.selected.iter().all(|skill| {
            let operations = skill.name == "rupu-session-operations";
            skill.layer == Layer::Global
                && skill.preload == operations
                && skill.indexed != operations
                && skill.content_hash.is_some() == operations
        }));
        assert!(plan.diagnostics.is_empty());
        assert_eq!(
            serde_json::from_str::<SkillPlan>(&serde_json::to_string(&plan).unwrap()).unwrap(),
            plan
        );
    }

    #[test]
    fn missing_optional_skills_and_opt_out_do_not_create_directories() {
        let temp = tempfile::tempdir().unwrap();
        let dirs = dirs(temp.path());
        let planned = plan(&dirs, true, &[], &[]).unwrap();
        assert!(planned.selected.is_empty());
        assert!(planned.preload_defaults.is_empty());
        assert_eq!(planned.diagnostics.len(), PRESET_ENTRIES.len());
        assert!(planned
            .diagnostics
            .iter()
            .all(|d| d.code == SkillDiagnosticCode::NotInstalled));
        let disabled = plan(&dirs, false, &[], &[]).unwrap();
        assert!(!disabled.preset.enabled);
        assert!(disabled.selection().is_empty());
        assert!(disabled.preload_defaults.is_empty());
        assert!(disabled.diagnostics.is_empty());
        assert!(frozen(&dirs, &planned.selected).ready);
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
    }

    #[test]
    fn explicit_project_preload_replaces_default_and_preserves_errors() {
        let temp = tempfile::tempdir().unwrap();
        let dirs = dirs(temp.path());
        install(dirs.global.as_deref().unwrap(), "coverage-assets", "");
        install(dirs.project.as_deref().unwrap(), "coverage-assets", "");
        let project = strings(&["project:coverage-assets"]);
        let planned = plan(&dirs, true, &[], &project).unwrap();
        assert!(planned.selection().is_empty());
        assert_eq!(planned.selected.len(), 1);
        assert_eq!(planned.selected[0].layer, Layer::Project);
        assert!(planned.selected[0].preload);
        for entries in [
            strings(&["coverage-assets"]),
            strings(&["global:coverage-assets", "project:coverage-assets"]),
            strings(&["global:missing"]),
        ] {
            assert!(matches!(
                plan(&dirs, true, &entries, &[]),
                Err(SkillPlanError::Resolve(_))
            ));
        }
        let duplicate = strings(&["global:coverage-assets", "global:coverage-assets"]);
        let planned = plan(&dirs, true, &duplicate, &[]).unwrap();
        assert_eq!(planned.selected.len(), 1);
        assert!(planned.selection().is_empty());
    }

    #[test]
    fn optional_invalid_and_manual_skills_are_reported_but_explicit_invalid_fails() {
        let temp = tempfile::tempdir().unwrap();
        let dirs = dirs(temp.path());
        let global = dirs.global.as_deref().unwrap();
        install(global, "coverage-assets", "");
        fs::write(global.join("coverage-assets/SKILL.md"), "broken").unwrap();
        install(global, "finding-report", "disable-model-invocation: true\n");
        let planned = plan(&dirs, true, &[], &[]).unwrap();
        assert!(planned.selected.is_empty());
        assert_eq!(planned.diagnostics[0].code, SkillDiagnosticCode::Invalid);
        assert_eq!(
            planned.diagnostics[1].code,
            SkillDiagnosticCode::NotModelLoadable
        );
        assert!(plan(&dirs, true, &strings(&["global:coverage-assets"]), &[]).is_err());
    }

    #[test]
    fn explicit_operations_selections_keep_the_requested_preload_flags() {
        let temp = tempfile::tempdir().unwrap();
        let dirs = dirs(temp.path());
        for root in [
            dirs.global.as_deref().unwrap(),
            dirs.project.as_deref().unwrap(),
        ] {
            install(root, "rupu-session-operations", "");
        }
        for entry in [
            "global:rupu-session-operations",
            "project:rupu-session-operations",
        ] {
            let entries = strings(&[entry]);
            let indexed = plan(&dirs, true, &entries, &[]).unwrap();
            assert!(indexed.defaults.is_empty());
            assert!(indexed.preload_defaults.is_empty());
            assert_eq!(indexed.selected.len(), 1);
            assert!(indexed.selected[0].indexed);
            assert!(!indexed.selected[0].preload);
            assert!(indexed.selected[0].content_hash.is_none());
            let preloaded = plan(&dirs, true, &[], &entries).unwrap();
            assert!(preloaded.defaults.is_empty());
            assert!(preloaded.preload_defaults.is_empty());
            assert_eq!(preloaded.selected.len(), 1);
            assert!(preloaded.selected[0].preload);
            assert!(!preloaded.selected[0].indexed);
            assert!(preloaded.selected[0].content_hash.is_some());
        }
    }

    #[test]
    fn project_only_installation_is_not_an_implicit_fallback() {
        let temp = tempfile::tempdir().unwrap();
        let dirs = dirs(temp.path());
        install(
            dirs.project.as_deref().unwrap(),
            "rupu-session-operations",
            "",
        );
        let planned = plan(&dirs, true, &[], &[]).unwrap();
        assert!(planned.selected.is_empty());
        assert!(planned.preload_defaults.is_empty());
        assert!(planned.diagnostics.iter().any(|diagnostic| {
            diagnostic.entry.as_deref() == Some("global:rupu-session-operations")
                && diagnostic.code == SkillDiagnosticCode::NotInstalled
        }));
    }

    #[test]
    fn explicit_cli_wildcards_keep_existing_grammar_without_becoming_preset_defaults() {
        let temp = tempfile::tempdir().unwrap();
        let dirs = dirs(temp.path());
        install(dirs.global.as_deref().unwrap(), "unrelated", "");
        let planned = plan(&dirs, true, &strings(&["global:*"]), &[]).unwrap();
        assert!(planned.selection().is_empty());
        assert_eq!(planned.selected.len(), 1);
        assert_eq!(planned.selected[0].name, "unrelated");
    }

    #[test]
    fn frozen_inspection_reports_change_removal_and_corruption_without_expansion() {
        let temp = tempfile::tempdir().unwrap();
        let dirs = dirs(temp.path());
        let global = dirs.global.as_deref().unwrap();
        install(global, "coverage-assets", "");
        let planned = plan(&dirs, true, &[], &[]).unwrap();
        install(global, "work-units", "");
        install(global, "rupu-session-operations", "");
        let source = global.join("coverage-assets/SKILL.md");
        let before_bytes = fs::read(&source).unwrap();
        let before_modified = fs::metadata(&source).unwrap().modified().unwrap();
        let before = frozen(&dirs, &planned.selected);
        assert!(before.ready);
        assert_eq!(before.entries.len(), 1);
        assert!(before.entries[0].model_loadable);
        assert_eq!(before.entries[0].changed, Some(false));
        assert_eq!(fs::read(&source).unwrap(), before_bytes);
        assert_eq!(
            fs::metadata(&source).unwrap().modified().unwrap(),
            before_modified
        );
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
        fs::write(global.join("coverage-assets/extra.txt"), "changed fixture").unwrap();
        let changed = frozen(&dirs, &planned.selected);
        assert!(changed.ready);
        assert_eq!(changed.entries[0].changed, Some(true));
        assert_eq!(changed.entries[0].frozen, planned.selected[0]);
        let mut corrupt = planned.selected.clone();
        corrupt[0].stamp = "not-a-stamp".into();
        let corrupt = frozen(&dirs, &corrupt);
        assert!(!corrupt.ready);
        assert_eq!(
            corrupt.entries[0].missing,
            Some(SkillDiagnosticCode::InvalidFrozenRecord)
        );
        fs::remove_dir_all(global.join("coverage-assets")).unwrap();
        let removed = frozen(&dirs, &planned.selected);
        assert!(!removed.ready);
        assert_eq!(removed.entries[0].installed, Some(false));
        assert_eq!(
            removed.entries[0].missing,
            Some(SkillDiagnosticCode::Removed)
        );
    }

    #[test]
    fn unreadable_layer_is_unknown_not_absent_or_available() {
        let temp = tempfile::tempdir().unwrap();
        let dirs = dirs(temp.path());
        let global = dirs.global.as_deref().unwrap();
        install(global, "coverage-assets", "");
        let planned = plan(&dirs, true, &[], &[]).unwrap();
        fs::remove_dir_all(global).unwrap();
        fs::write(global, "a file cannot be a skills directory").unwrap();
        assert!(matches!(
            plan(&dirs, true, &[], &[]),
            Err(SkillPlanError::Discovery {
                layer: Layer::Global,
                ..
            })
        ));
        let report = frozen(&dirs, &planned.selected);
        assert!(!report.ready);
        assert_eq!(report.entries[0].installed, None);
        assert_eq!(report.entries[0].valid, None);
        assert!(!report.entries[0].model_loadable);
        assert_eq!(
            report.entries[0].missing,
            Some(SkillDiagnosticCode::UnreadableLayer)
        );
    }
}

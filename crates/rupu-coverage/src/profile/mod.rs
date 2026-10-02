//! Engagement profiles: the data-driven packages that make the finding and
//! coverage layer agnostic to the *type of asset*. A profile declares asset
//! kinds (with their coordinates), the evidence blocks and taxonomies it uses,
//! a completeness checklist, a coverage depth ladder, and a launcher bundle.
//!
//! The core owns only the typed primitives (see [`crate::asset`] and
//! [`crate::report`]); a new engagement type is authored as data, not code.

pub mod loader;
pub mod package;
pub mod predicate;
pub mod registry;

pub use loader::{discover, expand_includes, include_closure, LoadError};
pub use package::{parse_profile, AssetKindDef, Bundle, Coverage, EngagementProfile, ProfileError};
pub use predicate::{evaluate, CompletenessCheck, Predicate, PredicateError};
pub use registry::{ActiveSet, ProfileRegistry, RegistryError};

use std::collections::BTreeMap;
use std::path::Path;

/// The built-in default engagement profile.
pub const DEFAULT_PROFILE: &str = "code";

/// Every profile shipped in the binary, keyed by id.
pub fn builtin_profiles() -> BTreeMap<String, EngagementProfile> {
    let mut raw = BTreeMap::new();
    for src in BUILTIN_TOMLS {
        let p = parse_profile(src).expect("embedded builtin profile parses");
        raw.insert(p.id.clone(), p);
    }
    raw
}

/// The embedded built-in profile packages. Add a new built-in by dropping its
/// `.toml` beside the others and listing it here.
const BUILTIN_TOMLS: &[&str] = &[include_str!("builtin/code.toml")];

/// The registry of just the built-in profiles.
pub fn builtin_registry() -> Result<ProfileRegistry, RegistryError> {
    ProfileRegistry::from_profiles(builtin_profiles())
}

/// The built-ins overlaid with `*.toml` profiles discovered in `global_dir`
/// then `project_dir` (a later source overrides an earlier one by id:
/// built-in < global < project). Fail-closed: a discovered file that does not
/// parse fails the whole overlay rather than silently running under the wrong
/// (or no) rules.
pub fn registry_with_overlay(
    global_dir: &Path,
    project_dir: &Path,
) -> Result<ProfileRegistry, RegistryError> {
    let (discovered, mut errors) = discover(&[global_dir.to_path_buf(), project_dir.to_path_buf()]);
    if !errors.is_empty() {
        errors.sort();
        return Err(RegistryError::Load(LoadError::MissingInclude(format!(
            "unloadable profiles: {}",
            errors.join("; ")
        ))));
    }
    let mut raw = builtin_profiles();
    raw.extend(discovered);
    ProfileRegistry::from_profiles(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_parse_and_code_routes() {
        let all = builtin_profiles();
        assert!(all.contains_key("code"));
        let reg = builtin_registry().unwrap();
        let set = reg.active_set(&[DEFAULT_PROFILE.into()]).unwrap();
        assert_eq!(set.profile_for_kind("code:file").unwrap().id, "code");
    }

    #[test]
    fn overlay_project_overrides_builtin_by_id() {
        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("global");
        let project = tmp.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        // a project profile that overrides `code`'s name
        std::fs::write(
            project.join("code.toml"),
            "id = \"code\"\nname = \"Custom code\"\n[[asset_kinds]]\nid=\"repo\"\nlabel=\"{path}\"\n",
        )
        .unwrap();
        let reg = registry_with_overlay(&global, &project).unwrap();
        let set = reg.active_set(&["code".into()]).unwrap();
        assert_eq!(
            set.profiles().find(|p| p.id == "code").unwrap().name,
            "Custom code"
        );
    }

    #[test]
    fn overlay_fails_closed_on_unparseable() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(project.join("x.toml"), "id=\"x\"\nbogus=1\n").unwrap();
        assert!(registry_with_overlay(tmp.path(), &project).is_err());
    }
}

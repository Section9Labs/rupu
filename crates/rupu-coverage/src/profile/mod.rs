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
/// `.toml` beside the others and listing it here. Composites (`mobile`,
/// `pentest`) must appear with the profiles they include.
const BUILTIN_TOMLS: &[&str] = &[
    include_str!("builtin/code.toml"),
    include_str!("builtin/binary.toml"),
    include_str!("builtin/firmware.toml"),
    include_str!("builtin/network.toml"),
    include_str!("builtin/web.toml"),
    include_str!("builtin/api.toml"),
    include_str!("builtin/cloud.toml"),
    include_str!("builtin/sca.toml"),
    include_str!("builtin/iac.toml"),
    include_str!("builtin/secrets.toml"),
    include_str!("builtin/container.toml"),
    include_str!("builtin/redteam.toml"),
    include_str!("builtin/threat-model.toml"),
    include_str!("builtin/mobile.toml"),
    include_str!("builtin/pentest.toml"),
];

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
    fn the_full_catalog_loads_and_each_profile_routes_its_root_kind() {
        let all = builtin_profiles();
        let expected = [
            "code",
            "binary",
            "firmware",
            "network",
            "web",
            "api",
            "cloud",
            "sca",
            "iac",
            "secrets",
            "container",
            "redteam",
            "threat-model",
            "mobile",
            "pentest",
        ];
        for id in expected {
            assert!(all.contains_key(id), "missing built-in profile `{id}`");
        }
        assert_eq!(all.len(), expected.len(), "unexpected extra built-ins");

        // The registry builds (composite includes resolve), and a representative
        // namespaced kind from each single-root profile routes to it.
        let reg = builtin_registry().unwrap();
        for (id, kind) in [
            ("network", "network:service"),
            ("web", "web:route"),
            ("api", "api:endpoint"),
            ("binary", "binary:function"),
            ("firmware", "firmware:function"),
            ("cloud", "cloud:resource"),
            ("sca", "sca:dependency"),
            ("iac", "iac:resource"),
            ("secrets", "secrets:secret"),
            ("container", "container:package"),
            ("redteam", "redteam:objective"),
            ("threat-model", "threat-model:component"),
        ] {
            let set = reg.active_set(&[id.to_string()]).unwrap();
            assert_eq!(set.profile_for_kind(kind).unwrap().id, id, "{kind}");
        }
    }

    #[test]
    fn composites_activate_members_and_route_per_origin() {
        let reg = builtin_registry().unwrap();
        // pentest = network + web
        let set = reg.active_set(&["pentest".into()]).unwrap();
        assert_eq!(
            set.profile_for_kind("network:service").unwrap().id,
            "network"
        );
        assert_eq!(set.profile_for_kind("web:route").unwrap().id, "web");
        // mobile = binary + web + its own `package` root
        let set = reg.active_set(&["mobile".into()]).unwrap();
        assert_eq!(
            set.profile_for_kind("binary:function").unwrap().id,
            "binary"
        );
        assert_eq!(set.profile_for_kind("web:route").unwrap().id, "web");
        assert_eq!(set.profile_for_kind("mobile:package").unwrap().id, "mobile");
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

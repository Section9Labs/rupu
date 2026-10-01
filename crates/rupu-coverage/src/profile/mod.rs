//! Engagement profiles: data-driven asset families layered on the primitives.
pub mod loader;
pub mod package;
pub mod predicate;
pub mod registry;

pub use loader::{discover, expand_includes, include_closure, LoadError};
pub use package::{
    parse_profile, AssetKindDef, Bundle, CoverageSpec, EngagementProfile, ProfileError,
};
pub use predicate::{score, CompletenessCheck, Predicate, PredicateError};
pub use registry::{ActiveSet, ProfileRegistry, RegistryError};

pub const DEFAULT_PROFILE: &str = "code";

pub fn code_profile() -> EngagementProfile {
    EngagementProfile {
        id: "code".into(),
        name: "Secure code review".into(),
        includes: vec![],
        asset_kinds: vec![AssetKindDef {
            id: "file".into(),
            parent: None,
            coordinates: vec!["path".into(), "line_range".into(), "symbol".into()],
            attributes: vec![],
            label: "{path}".into(),
        }],
        evidence_blocks: vec!["text".into(), "code_slice".into(), "diff".into()],
        classification_systems: vec!["CWE".into()],
        completeness: vec![], // native path is unchanged; no profile-driven checks yet
        coverage: CoverageSpec {
            enumerates: vec!["file".into()],
            depth_ladder: vec!["unreviewed".into(), "reviewed".into()],
        },
        bundle: Bundle::default(),
    }
}

impl EngagementProfile {
    /// Which engagement profiles a run activates. Precedence, most specific
    /// first: step -> workflow `defaults` -> agent frontmatter -> the built-in
    /// default ([`DEFAULT_PROFILE`]). The first non-empty level wins whole;
    /// levels are never merged. A blank step is "unset".
    pub fn resolve(step: Option<&str>, workflow: &[String], agent: &[String]) -> Vec<String> {
        if let Some(s) = step.filter(|s| !s.trim().is_empty()) {
            return vec![s.to_string()];
        }
        if !workflow.is_empty() {
            return workflow.to_vec();
        }
        if !agent.is_empty() {
            return agent.to_vec();
        }
        vec![DEFAULT_PROFILE.to_string()]
    }
}

/// The profiles shipped in the binary, keyed by id.
pub fn builtin_profiles() -> std::collections::BTreeMap<String, EngagementProfile> {
    let mut raw = std::collections::BTreeMap::new();
    raw.insert("code".to_string(), code_profile());
    let binary = package::parse_profile(include_str!("builtin/binary.toml"))
        .expect("embedded binary profile parses");
    raw.insert("binary".to_string(), binary);
    raw
}

pub fn builtin_registry() -> Result<registry::ProfileRegistry, registry::RegistryError> {
    registry::ProfileRegistry::from_profiles(builtin_profiles())
}

/// The built-ins overlaid with `*.toml` profiles discovered in `global_dir`
/// then `project_dir` (the profile directories themselves, e.g.
/// `<global>/profiles` and `<project>/.rupu/profiles`; a missing directory is
/// skipped). A later source overrides an earlier one by id: built-ins <
/// global < project.
///
/// Fail-closed: a discovered file that does not parse is an
/// [`RegistryError::InvalidProfiles`] — a profile that silently failed to load
/// would otherwise leave an engagement running under the wrong (or no)
/// completeness rules. Any such file fails the overlay, since an unparseable
/// file cannot be attributed to the id it would have defined.
pub fn registry_with_overlay(
    global_dir: &std::path::Path,
    project_dir: &std::path::Path,
) -> Result<registry::ProfileRegistry, registry::RegistryError> {
    let (discovered, mut errors) =
        loader::discover(&[global_dir.to_path_buf(), project_dir.to_path_buf()]);
    if !errors.is_empty() {
        errors.sort();
        return Err(registry::RegistryError::InvalidProfiles(errors));
    }
    let mut raw = builtin_profiles();
    raw.extend(discovered);
    registry::ProfileRegistry::from_profiles(raw)
}

#[cfg(test)]
mod builtin_tests {
    use super::*;

    #[test]
    fn builtin_registry_has_code_and_binary_namespaced() {
        let reg = builtin_registry().unwrap();
        let set = reg.active_set(&["code".into(), "binary".into()]).unwrap();
        assert_eq!(
            set.profile_for_kind("binary:function").unwrap().id,
            "binary"
        );
        assert_eq!(set.profile_for_kind("code:file").unwrap().id, "code");
    }

    #[test]
    fn binary_profile_completeness_requires_a_listing() {
        let reg = builtin_registry().unwrap();
        let set = reg.active_set(&["binary".into()]).unwrap();
        let bin = set.profile_for_kind("binary:function").unwrap();
        assert!(bin
            .completeness
            .iter()
            .any(|c| c.id == "evidence_has_listing"));
    }
}

#[cfg(test)]
mod resolve_and_overlay_tests {
    use super::*;
    use std::path::Path;

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn resolve_prefers_most_specific() {
        let wf = ids(&["binary"]);
        let ag = ids(&["web"]);
        // step > workflow defaults > agent frontmatter > built-in default.
        assert_eq!(
            EngagementProfile::resolve(Some("network"), &wf, &ag),
            ids(&["network"])
        );
        assert_eq!(EngagementProfile::resolve(None, &wf, &ag), wf);
        assert_eq!(EngagementProfile::resolve(None, &[], &ag), ag);
        assert_eq!(
            EngagementProfile::resolve(None, &[], &[]),
            vec![DEFAULT_PROFILE.to_string()]
        );
        // An empty / blank step is "unset", not an override.
        assert_eq!(EngagementProfile::resolve(Some(""), &wf, &ag), wf);
        assert_eq!(EngagementProfile::resolve(Some("  "), &[], &ag), ag);
        // A non-empty list wins whole — lists are never merged across levels.
        let many = ids(&["binary", "web"]);
        assert_eq!(EngagementProfile::resolve(None, &many, &ag), many);
    }

    fn write(dir: &Path, name: &str, body: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(name), body).unwrap();
    }

    fn toml(id: &str, kind: &str, includes: &[&str]) -> String {
        let inc = includes
            .iter()
            .map(|i| format!("\"{i}\""))
            .collect::<Vec<_>>()
            .join(",");
        format!(
            "id=\"{id}\"\nname=\"{id}\"\nincludes=[{inc}]\n\
             [[asset_kinds]]\nid=\"{kind}\"\nlabel=\"l\"\n[coverage]\n[bundle]\n"
        )
    }

    #[test]
    fn overlay_without_profile_files_is_the_builtin_registry() {
        let tmp = tempfile::tempdir().unwrap();
        // Neither directory exists.
        let reg = registry_with_overlay(&tmp.path().join("g"), &tmp.path().join("p")).unwrap();
        let set = reg.active_set(&["code".into(), "binary".into()]).unwrap();
        assert_eq!(set.profile_for_kind("code:file").unwrap().id, "code");
        assert_eq!(
            set.profile_for_kind("binary:function").unwrap().id,
            "binary"
        );
    }

    #[test]
    fn overlay_folds_discovered_profiles_over_builtins_project_last() {
        let tmp = tempfile::tempdir().unwrap();
        let (g, p) = (tmp.path().join("global"), tmp.path().join("project"));
        write(&g, "network.toml", &toml("network", "service", &[]));
        write(&g, "web.toml", &toml("web", "route", &[]));
        // The project redefines `web`: it wins over the global one.
        write(&p, "web.toml", &toml("web", "page", &[]));
        // A composite authored as a file routes per origin.
        write(
            &p,
            "pentest.toml",
            &toml("pentest", "scope", &["network", "web"]),
        );
        // A discovered file may also override a built-in by id.
        write(&g, "binary.toml", &toml("binary", "blob", &[]));

        let reg = registry_with_overlay(&g, &p).unwrap();
        let set = reg.active_set(&["pentest".into()]).unwrap();
        assert_eq!(
            set.profile_for_kind("network:service").unwrap().id,
            "network"
        );
        assert_eq!(set.profile_for_kind("web:page").unwrap().id, "web");
        assert_eq!(
            set.profile_for_kind("web:page").unwrap().asset_kinds[0].id,
            "web:page"
        );
        let bin = reg.active_set(&["binary".into()]).unwrap();
        assert_eq!(
            bin.profile_for_kind("binary:blob").unwrap().asset_kinds[0].id,
            "binary:blob"
        );
        // Built-ins the overlay did not touch are still there.
        assert!(reg.active_set(&["code".into()]).is_ok());
    }

    #[test]
    fn overlay_fails_closed_on_bad_profile() {
        let tmp = tempfile::tempdir().unwrap();
        let (g, p) = (tmp.path().join("global"), tmp.path().join("project"));
        write(&g, "network.toml", &toml("network", "service", &[]));
        write(&p, "broken.toml", "id = \"broken\"\nthis is not toml ===\n");

        // A malformed profile file is an error, never silently dropped — even
        // though every valid profile alongside it would have loaded fine.
        let err = registry_with_overlay(&g, &p)
            .err()
            .expect("must fail closed");
        assert!(matches!(err, RegistryError::InvalidProfiles(ref v) if v.len() == 1));
        let msg = err.to_string();
        assert!(msg.contains("broken.toml"), "{msg}");

        // An unknown coordinate is a parse error too.
        let tmp2 = tempfile::tempdir().unwrap();
        write(
            tmp2.path(),
            "bad.toml",
            "id=\"bad\"\nname=\"bad\"\n[[asset_kinds]]\nid=\"k\"\ncoordinates=[\"nonsense\"]\nlabel=\"l\"\n[coverage]\n[bundle]\n",
        );
        assert!(matches!(
            registry_with_overlay(tmp2.path(), &tmp2.path().join("none")),
            Err(RegistryError::InvalidProfiles(_))
        ));
    }

    #[test]
    fn overlay_surfaces_include_errors_from_discovered_files() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            tmp.path(),
            "pentest.toml",
            &toml("pentest", "scope", &["missing"]),
        );
        assert!(matches!(
            registry_with_overlay(tmp.path(), &tmp.path().join("none")),
            Err(RegistryError::Load(LoadError::MissingInclude(_)))
        ));
    }
}

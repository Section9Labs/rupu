//! Engagement profiles: data-driven asset families layered on the primitives.
pub mod loader;
pub mod package;
pub mod predicate;
pub mod registry;

pub use loader::{discover, expand_includes, LoadError};
pub use package::{parse_profile, AssetKindDef, Bundle, CoverageSpec, EngagementProfile, ProfileError};
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

pub fn builtin_registry() -> Result<registry::ProfileRegistry, registry::RegistryError> {
	let mut raw = std::collections::BTreeMap::new();
	raw.insert("code".to_string(), code_profile());
	let binary = package::parse_profile(include_str!("builtin/binary.toml"))
		.expect("embedded binary profile parses");
	raw.insert("binary".to_string(), binary);
	registry::ProfileRegistry::from_profiles(raw)
}

#[cfg(test)]
mod builtin_tests {
	use super::*;

	#[test]
	fn builtin_registry_has_code_and_binary_namespaced() {
		let reg = builtin_registry().unwrap();
		let set = reg.active_set(&["code".into(), "binary".into()]).unwrap();
		assert_eq!(set.profile_for_kind("binary:function").unwrap().id, "binary");
		assert_eq!(set.profile_for_kind("code:file").unwrap().id, "code");
	}

	#[test]
	fn binary_profile_completeness_requires_a_listing() {
		let reg = builtin_registry().unwrap();
		let set = reg.active_set(&["binary".into()]).unwrap();
		let bin = set.profile_for_kind("binary:function").unwrap();
		assert!(bin.completeness.iter().any(|c| c.id == "evidence_has_listing"));
	}
}

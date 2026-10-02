//! The profile registry and the per-run active set.
//!
//! Every profile — base or composite — is registered individually and routable
//! by its OWN namespace (`network:service`). A composite is a *selection
//! shorthand*, not a merged profile: selecting `pentest` activates `network`
//! and `web` as separate routing targets, so a `network:service` finding is
//! validated against `network`'s own completeness/taxonomy, never a merged
//! union.

use super::loader::{dedup, include_closure, LoadError};
use super::package::EngagementProfile;
use std::collections::{BTreeMap, BTreeSet};

/// Errors building a registry or resolving an active set.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RegistryError {
    #[error("unknown engagement profile `{0}`")]
    UnknownProfile(String),
    #[error("profile load: {0}")]
    Load(#[from] LoadError),
    #[error("asset kind `{0}` is contributed by more than one selected profile")]
    KindCollision(String),
    #[error("profile `{0}` is not in the active set (a step may narrow, never widen)")]
    WidenNotAllowed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    profile: EngagementProfile,
    /// `id` itself, then every profile it transitively includes.
    closure: Vec<String>,
}

/// Every registered profile, routable by its own namespace.
pub struct ProfileRegistry {
    entries: BTreeMap<String, Entry>,
}

impl ProfileRegistry {
    /// Build the registry from raw (un-flattened) profiles keyed by id. Each
    /// profile's asset kinds are namespaced `"<id>:<kind>"`; a composite is NOT
    /// flattened — it keeps its own (usually empty) declarations and a closure.
    pub fn from_profiles(raw: BTreeMap<String, EngagementProfile>) -> Result<Self, RegistryError> {
        let mut entries = BTreeMap::new();
        for (id, base) in &raw {
            let closure = include_closure(&raw, id)?;
            let mut own = base.clone();
            own.id = id.clone();
            own.includes.clear();
            dedup(&mut own);
            for k in &mut own.asset_kinds {
                k.id = format!("{id}:{}", k.id);
                if let Some(parent) = &mut k.parent {
                    *parent = format!("{id}:{parent}");
                }
            }
            entries.insert(
                id.clone(),
                Entry {
                    profile: own,
                    closure,
                },
            );
        }
        Ok(ProfileRegistry { entries })
    }

    /// Activate `ids`. Each id contributes the origin profiles it expands to;
    /// the result's `ids()` stays the selected labels. A kind contributed by
    /// more than one selected id is a collision (fail-closed).
    pub fn active_set(&self, ids: &[String]) -> Result<ActiveSet, RegistryError> {
        let mut entries = BTreeMap::new();
        let mut seen = BTreeSet::new();
        for id in ids {
            let entry = self
                .entries
                .get(id)
                .ok_or_else(|| RegistryError::UnknownProfile(id.clone()))?;
            let mut contributed = BTreeSet::new();
            for member in &entry.closure {
                let m = self
                    .entries
                    .get(member)
                    .ok_or_else(|| RegistryError::UnknownProfile(member.clone()))?;
                contributed.extend(m.profile.asset_kinds.iter().map(|k| k.id.clone()));
                entries.insert(member.clone(), m.clone());
            }
            for kind in contributed {
                if !seen.insert(kind.clone()) {
                    return Err(RegistryError::KindCollision(kind));
                }
            }
        }
        Ok(ActiveSet {
            selected: ids.to_vec(),
            entries,
        })
    }
}

/// The profiles active for a run, routable by namespaced kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveSet {
    selected: Vec<String>,
    entries: BTreeMap<String, Entry>,
}

impl ActiveSet {
    /// The ids the caller selected (a composite appears as itself).
    pub fn ids(&self) -> Vec<&str> {
        self.selected.iter().map(String::as_str).collect()
    }

    /// Every active ORIGIN profile (selected ids plus everything they
    /// transitively include), each once, in id order.
    pub fn profiles(&self) -> impl Iterator<Item = &EngagementProfile> {
        self.entries.values().map(|e| &e.profile)
    }

    /// The origin profile that owns `namespaced_kind` (`network:service` =>
    /// `network`), with that profile's own completeness/taxonomy — whether it
    /// was selected directly or through a composite.
    pub fn profile_for_kind(&self, namespaced_kind: &str) -> Option<&EngagementProfile> {
        let prof = crate::asset::profile_of(namespaced_kind);
        self.entries.get(prof).map(|e| &e.profile)
    }

    /// Narrow to `subset`. Each id must already be active (selected, or an
    /// origin reachable from a selected composite). Anything else is a
    /// widening and fails.
    pub fn narrow(&self, subset: &[String]) -> Result<ActiveSet, RegistryError> {
        let mut entries = BTreeMap::new();
        for id in subset {
            let entry = self
                .entries
                .get(id)
                .ok_or_else(|| RegistryError::WidenNotAllowed(id.clone()))?;
            entries.insert(id.clone(), entry.clone());
        }
        Ok(ActiveSet {
            selected: subset.to_vec(),
            entries,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::package::parse_profile;

    fn p(id: &str, kind: &str) -> EngagementProfile {
        parse_profile(&format!(
            "id=\"{id}\"\nname=\"{id}\"\n[[asset_kinds]]\nid=\"{kind}\"\nlabel=\"l\"\n"
        ))
        .unwrap()
    }

    fn composite(id: &str, includes: &[&str]) -> EngagementProfile {
        let inc = includes
            .iter()
            .map(|i| format!("\"{i}\""))
            .collect::<Vec<_>>()
            .join(", ");
        parse_profile(&format!("id=\"{id}\"\nname=\"{id}\"\nincludes=[{inc}]\n")).unwrap()
    }

    fn registry() -> ProfileRegistry {
        let mut all = BTreeMap::new();
        all.insert("network".into(), p("network", "service"));
        all.insert("web".into(), p("web", "route"));
        all.insert("pentest".into(), composite("pentest", &["network", "web"]));
        ProfileRegistry::from_profiles(all).unwrap()
    }

    #[test]
    fn routes_by_namespaced_kind_through_a_composite() {
        let reg = registry();
        let set = reg.active_set(&["pentest".into()]).unwrap();
        assert_eq!(set.ids(), ["pentest"]);
        assert_eq!(
            set.profile_for_kind("network:service").unwrap().id,
            "network"
        );
        assert_eq!(set.profile_for_kind("web:route").unwrap().id, "web");
        assert!(set.profile_for_kind("cloud:bucket").is_none());
    }

    #[test]
    fn narrow_only_never_widens() {
        let reg = registry();
        let set = reg.active_set(&["pentest".into()]).unwrap();
        let narrowed = set.narrow(&["network".into()]).unwrap();
        assert!(narrowed.profile_for_kind("web:route").is_none());
        assert!(narrowed.profile_for_kind("network:service").is_some());
        assert!(matches!(
            set.narrow(&["cloud".into()]),
            Err(RegistryError::WidenNotAllowed(_))
        ));
    }

    #[test]
    fn unknown_profile_and_kind_collision_are_errors() {
        let reg = registry();
        assert!(matches!(
            reg.active_set(&["ghost".into()]),
            Err(RegistryError::UnknownProfile(_))
        ));
        // selecting a composite alongside one of its own members double-counts
        // a kind -> collision.
        assert!(matches!(
            reg.active_set(&["pentest".into(), "network".into()]),
            Err(RegistryError::KindCollision(_))
        ));
    }
}

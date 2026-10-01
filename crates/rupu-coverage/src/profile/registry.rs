use crate::asset::types::profile_of;
use crate::profile::loader::{expand_includes, LoadError};
use crate::profile::EngagementProfile;
use std::collections::BTreeMap;

#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("unknown engagement profile: {0}")]
    UnknownProfile(String),
    #[error("kind {0:?} is declared by more than one active profile")]
    KindCollision(String),
    #[error("a step may only narrow the active set; {0:?} is not in it")]
    WidenNotAllowed(String),
    #[error(transparent)]
    Load(#[from] LoadError),
}

pub struct ProfileRegistry {
    /// Flattened (composites expanded), kinds namespaced `<id>:<kind>`.
    profiles: BTreeMap<String, EngagementProfile>,
}

impl ProfileRegistry {
    pub fn from_profiles(raw: BTreeMap<String, EngagementProfile>) -> Result<Self, RegistryError> {
        let mut profiles = BTreeMap::new();
        for id in raw.keys() {
            let mut flat = expand_includes(&raw, id)?;
            for k in &mut flat.asset_kinds {
                k.id = format!("{}:{}", flat.id, k.id);
                if let Some(parent) = &mut k.parent {
                    *parent = format!("{}:{}", flat.id, parent);
                }
            }
            profiles.insert(id.clone(), flat);
        }
        Ok(ProfileRegistry { profiles })
    }

    pub fn active_set(&self, ids: &[String]) -> Result<ActiveSet, RegistryError> {
        let mut chosen = Vec::new();
        for id in ids {
            let p = self
                .profiles
                .get(id)
                .ok_or_else(|| RegistryError::UnknownProfile(id.clone()))?;
            chosen.push(p.clone());
        }
        // Fail-closed on a namespaced-kind collision across the active set.
        let mut seen = std::collections::HashSet::new();
        for p in &chosen {
            for k in &p.asset_kinds {
                if !seen.insert(k.id.clone()) {
                    return Err(RegistryError::KindCollision(k.id.clone()));
                }
            }
        }
        Ok(ActiveSet { profiles: chosen })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveSet {
    profiles: Vec<EngagementProfile>,
}

impl ActiveSet {
    pub fn ids(&self) -> Vec<&str> {
        self.profiles.iter().map(|p| p.id.as_str()).collect()
    }

    pub fn profile_for_kind(&self, namespaced_kind: &str) -> Option<&EngagementProfile> {
        let owner = profile_of(namespaced_kind);
        self.profiles.iter().find(|p| p.id == owner)
    }

    pub fn narrow(&self, subset: &[String]) -> Result<ActiveSet, RegistryError> {
        let mut kept = Vec::new();
        for id in subset {
            match self.profiles.iter().find(|p| &p.id == id) {
                Some(p) => kept.push(p.clone()),
                None => return Err(RegistryError::WidenNotAllowed(id.clone())),
            }
        }
        Ok(ActiveSet { profiles: kept })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::parse_profile;
    use std::collections::BTreeMap;

    fn p(id: &str, kind: &str) -> crate::profile::EngagementProfile {
        parse_profile(&format!("id=\"{id}\"\nname=\"{id}\"\n[[asset_kinds]]\nid=\"{kind}\"\nlabel=\"l\"\n[coverage]\n[bundle]\n")).unwrap()
    }

    #[test]
    fn routes_by_namespaced_kind_and_narrows_only() {
        let mut all = BTreeMap::new();
        all.insert("network".into(), p("network", "service"));
        all.insert("web".into(), p("web", "route"));
        let reg = ProfileRegistry::from_profiles(all).unwrap();

        let set = reg.active_set(&["network".into(), "web".into()]).unwrap();
        assert_eq!(
            set.profile_for_kind("network:service").unwrap().id,
            "network"
        );
        assert_eq!(set.profile_for_kind("web:route").unwrap().id, "web");
        assert!(set.profile_for_kind("cloud:bucket").is_none());

        let narrowed = set.narrow(&["network".into()]).unwrap();
        assert!(narrowed.profile_for_kind("web:route").is_none());
        assert!(matches!(
            set.narrow(&["cloud".into()]),
            Err(RegistryError::WidenNotAllowed(_))
        ));
    }

    #[test]
    fn unknown_profile_errors() {
        let reg = ProfileRegistry::from_profiles(BTreeMap::new()).unwrap();
        assert!(matches!(
            reg.active_set(&["ghost".into()]),
            Err(RegistryError::UnknownProfile(_))
        ));
    }

    #[test]
    fn active_set_is_clone_and_eq() {
        let mut all = BTreeMap::new();
        all.insert("network".into(), p("network", "service"));
        let reg = ProfileRegistry::from_profiles(all).unwrap();
        let set = reg.active_set(&["network".into()]).unwrap();
        let copy = set.clone();
        assert_eq!(set, copy);
        let empty = reg.active_set(&[]).unwrap();
        assert_ne!(set, empty);
    }
}

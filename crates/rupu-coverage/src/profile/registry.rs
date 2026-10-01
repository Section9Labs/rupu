use crate::asset::types::profile_of;
use crate::profile::loader::{dedup, include_closure, LoadError};
use crate::profile::EngagementProfile;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("unknown engagement profile: {0}")]
    UnknownProfile(String),
    #[error("kind {0:?} is declared by more than one active profile")]
    KindCollision(String),
    #[error("a step may only narrow the active set; {0:?} is not in it")]
    WidenNotAllowed(String),
    #[error("invalid engagement profile file(s): {}", fmt_invalid(.0))]
    InvalidProfiles(Vec<(PathBuf, String)>),
    #[error(transparent)]
    Load(#[from] LoadError),
}

fn fmt_invalid(errs: &[(PathBuf, String)]) -> String {
    errs.iter()
        .map(|(path, msg)| format!("{}: {msg}", path.display()))
        .collect::<Vec<_>>()
        .join("; ")
}

/// One routable profile: its OWN definition (includes not merged, kinds
/// namespaced `<id>:<kind>`) plus the ids it expands to when it is selected.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    profile: EngagementProfile,
    /// `id` itself, then every profile it transitively includes (each once).
    closure: Vec<String>,
}

/// Every profile — base or composite — registered individually and routable
/// by its own namespace.
///
/// A composite (`pentest` = includes `network`, `web`) is a *selection
/// shorthand*, not a merged profile: selecting it activates each included
/// profile as its own routing target, so a finding on `network:service` is
/// validated against `network`'s completeness/taxonomy whether `network` or
/// `pentest` was selected. Nothing is ever validated against a composite's
/// merged union.
pub struct ProfileRegistry {
    entries: BTreeMap<String, Entry>,
}

impl ProfileRegistry {
    /// Build the registry from raw (un-flattened) profiles keyed by id. The
    /// key is the authoritative id (includes reference it, selection uses it).
    pub fn from_profiles(raw: BTreeMap<String, EngagementProfile>) -> Result<Self, RegistryError> {
        let mut entries = BTreeMap::new();
        for (id, base) in &raw {
            // Validates cycles / missing includes for every profile.
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

    /// Activate `ids`. Each id (composite or not) contributes the origin
    /// profiles it expands to; `ids()` of the result stays the selected labels.
    pub fn active_set(&self, ids: &[String]) -> Result<ActiveSet, RegistryError> {
        let mut entries = BTreeMap::new();
        // Fail-closed on a namespaced-kind collision across the selections: a
        // kind may be contributed by only one selected id, so a repeated id or
        // a composite selected alongside one of its own members is rejected
        // rather than silently merged.
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
    /// The ids the caller selected (composites kept as the label).
    selected: Vec<String>,
    /// Every routable origin profile (the selected ids plus everything they
    /// transitively include), by id.
    entries: BTreeMap<String, Entry>,
}

impl ActiveSet {
    /// The selected ids (a composite appears as itself, not its members).
    pub fn ids(&self) -> Vec<&str> {
        self.selected.iter().map(String::as_str).collect()
    }

    /// The ORIGIN profile that owns `namespaced_kind` (`network:service` =>
    /// `network`), with that profile's own completeness/taxonomy — whether it
    /// was selected directly or through a composite.
    pub fn profile_for_kind(&self, namespaced_kind: &str) -> Option<&EngagementProfile> {
        self.entries
            .get(profile_of(namespaced_kind))
            .map(|e| &e.profile)
    }

    /// Narrow to `subset`. Each id must already be active — selected, or an
    /// origin reachable from a selected composite (narrowing `pentest` to
    /// `network` is a narrowing, not a widening). Anything else is a widening
    /// and fails.
    pub fn narrow(&self, subset: &[String]) -> Result<ActiveSet, RegistryError> {
        let mut entries = BTreeMap::new();
        for id in subset {
            let entry = self
                .entries
                .get(id)
                .ok_or_else(|| RegistryError::WidenNotAllowed(id.clone()))?;
            for member in &entry.closure {
                if let Some(m) = self.entries.get(member) {
                    entries.insert(member.clone(), m.clone());
                }
            }
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

    /// A profile with one kind, one required completeness check, one taxonomy
    /// and one evidence block, all named after the profile, so a test can tell
    /// whose rules it got back (and that nothing was unioned in).
    fn pc(id: &str, kind: &str) -> crate::profile::EngagementProfile {
        parse_profile(&format!(
            "id=\"{id}\"\nname=\"{id}\"\nclassification_systems=[\"{id}-taxonomy\"]\n\
             evidence_blocks=[\"{id}-block\"]\n\
             [[asset_kinds]]\nid=\"{kind}\"\nlabel=\"l\"\n\
             [[completeness]]\nid=\"{id}_check\"\nlabel=\"l\"\nrequired=true\n\
             satisfied_when = {{ has_field = \"root_cause\" }}\n\
             [coverage]\n[bundle]\n"
        ))
        .unwrap()
    }

    fn with_includes(
        mut p: crate::profile::EngagementProfile,
        includes: &[&str],
    ) -> crate::profile::EngagementProfile {
        p.includes = includes.iter().map(|s| s.to_string()).collect();
        p
    }

    /// network{service}, web{route}, pentest = includes[network, web] + its own `scope`.
    fn pentest_registry() -> ProfileRegistry {
        let mut all = BTreeMap::new();
        all.insert("network".into(), pc("network", "service"));
        all.insert("web".into(), pc("web", "route"));
        all.insert(
            "pentest".into(),
            with_includes(pc("pentest", "scope"), &["network", "web"]),
        );
        ProfileRegistry::from_profiles(all).unwrap()
    }

    fn check_ids(p: &crate::profile::EngagementProfile) -> Vec<&str> {
        p.completeness.iter().map(|c| c.id.as_str()).collect()
    }

    #[test]
    fn composite_routes_to_origin_subprofile() {
        let reg = pentest_registry();

        // Selected via the composite: each included kind routes to the profile
        // it ORIGINATED from, with that profile's OWN completeness/taxonomy —
        // never the composite's merged union.
        let set = reg.active_set(&["pentest".into()]).unwrap();
        let net = set.profile_for_kind("network:service").unwrap();
        assert_eq!(net.id, "network");
        assert_eq!(check_ids(net), vec!["network_check"]);
        assert_eq!(net.classification_systems, vec!["network-taxonomy"]);
        assert_eq!(net.evidence_blocks, vec!["network-block"]);
        let web = set.profile_for_kind("web:route").unwrap();
        assert_eq!(web.id, "web");
        assert_eq!(check_ids(web), vec!["web_check"]);
        assert_eq!(web.classification_systems, vec!["web-taxonomy"]);
        assert_eq!(web.evidence_blocks, vec!["web-block"]);
        // The composite's own kind routes to the composite's OWN definition.
        let own = set.profile_for_kind("pentest:scope").unwrap();
        assert_eq!(own.id, "pentest");
        assert_eq!(check_ids(own), vec!["pentest_check"]);
        assert_eq!(own.classification_systems, vec!["pentest-taxonomy"]);
        assert_eq!(own.evidence_blocks, vec!["pentest-block"]);
        assert!(own.includes.is_empty());
        // Kinds the composite does not reach are still unowned.
        assert!(set.profile_for_kind("cloud:bucket").is_none());
        // The selected label stays the composite.
        assert_eq!(set.ids(), vec!["pentest"]);

        // Selected directly: the same origin profile, byte for byte.
        let direct = reg.active_set(&["network".into()]).unwrap();
        let direct_net = direct.profile_for_kind("network:service").unwrap();
        assert_eq!(direct_net.id, "network");
        assert_eq!(direct_net, net);
        assert!(direct.profile_for_kind("web:route").is_none());
        assert_eq!(direct.ids(), vec!["network"]);
    }

    #[test]
    fn origin_profiles_own_namespaced_kinds_and_parents() {
        let mut all = BTreeMap::new();
        let mut net = pc("network", "host");
        net.asset_kinds.push(crate::profile::AssetKindDef {
            id: "service".into(),
            parent: Some("host".into()),
            coordinates: vec![],
            attributes: vec![],
            label: "l".into(),
        });
        all.insert("network".into(), net);
        all.insert(
            "pentest".into(),
            with_includes(pc("pentest", "scope"), &["network"]),
        );
        let reg = ProfileRegistry::from_profiles(all).unwrap();
        let set = reg.active_set(&["pentest".into()]).unwrap();
        let net = set.profile_for_kind("network:service").unwrap();
        let kinds: Vec<_> = net.asset_kinds.iter().map(|k| k.id.as_str()).collect();
        assert_eq!(kinds, vec!["network:host", "network:service"]);
        assert_eq!(
            net.asset_kinds[1].parent.as_deref(),
            Some("network:host"),
            "a parent stays inside its own origin's namespace"
        );
    }

    #[test]
    fn nested_composites_and_diamonds_route_each_origin_once() {
        let mut all = BTreeMap::new();
        all.insert("network".into(), pc("network", "service"));
        all.insert("web".into(), pc("web", "route"));
        all.insert(
            "pentest".into(),
            with_includes(pc("pentest", "scope"), &["network", "web"]),
        );
        // `full` reaches `network` twice: through `pentest` and directly.
        all.insert(
            "full".into(),
            with_includes(pc("full", "report"), &["pentest", "network"]),
        );
        let reg = ProfileRegistry::from_profiles(all).unwrap();
        let set = reg.active_set(&["full".into()]).unwrap();
        for (kind, owner) in [
            ("network:service", "network"),
            ("web:route", "web"),
            ("pentest:scope", "pentest"),
            ("full:report", "full"),
        ] {
            assert_eq!(set.profile_for_kind(kind).unwrap().id, owner, "{kind}");
        }
    }

    #[test]
    fn narrowing_a_composite_to_an_origin_is_not_widening() {
        let reg = pentest_registry();
        let set = reg.active_set(&["pentest".into()]).unwrap();

        // network is reachable from pentest, so it is a narrowing.
        let net_only = set.narrow(&["network".into()]).unwrap();
        assert_eq!(net_only.ids(), vec!["network"]);
        assert_eq!(
            net_only.profile_for_kind("network:service").unwrap().id,
            "network"
        );
        assert!(net_only.profile_for_kind("web:route").is_none());
        assert!(net_only.profile_for_kind("pentest:scope").is_none());
        // ...and once narrowed it cannot be widened back.
        assert!(matches!(
            net_only.narrow(&["web".into()]),
            Err(RegistryError::WidenNotAllowed(_))
        ));

        // A profile outside the composite is a widening.
        assert!(matches!(
            set.narrow(&["cloud".into()]),
            Err(RegistryError::WidenNotAllowed(_))
        ));
        // Narrowing keeps the whole composite when the composite is kept.
        let same = set.narrow(&["pentest".into()]).unwrap();
        assert_eq!(same, set);
    }

    #[test]
    fn kind_collision_fires_on_a_true_duplicate() {
        let reg = pentest_registry();
        // The same profile twice declares `network:service` twice.
        assert!(matches!(
            reg.active_set(&["network".into(), "network".into()]),
            Err(RegistryError::KindCollision(k)) if k == "network:service"
        ));
        // A composite plus one of its own members declares it twice as well.
        assert!(matches!(
            reg.active_set(&["pentest".into(), "network".into()]),
            Err(RegistryError::KindCollision(k)) if k == "network:service"
        ));
        // Disjoint selections do not collide.
        assert!(reg.active_set(&["network".into(), "web".into()]).is_ok());
    }

    #[test]
    fn missing_include_and_cycles_fail_registry_construction() {
        let mut m = BTreeMap::new();
        m.insert("x".into(), with_includes(pc("x", "kx"), &["missing"]));
        assert!(matches!(
            ProfileRegistry::from_profiles(m),
            Err(RegistryError::Load(LoadError::MissingInclude(_)))
        ));
        let mut c = BTreeMap::new();
        c.insert("a".into(), with_includes(pc("a", "ka"), &["b"]));
        c.insert("b".into(), with_includes(pc("b", "kb"), &["a"]));
        assert!(matches!(
            ProfileRegistry::from_profiles(c),
            Err(RegistryError::Load(LoadError::IncludeCycle(_)))
        ));
    }

    #[test]
    fn the_registry_key_is_the_authoritative_profile_id() {
        let mut m = BTreeMap::new();
        m.insert("net".into(), pc("network", "service"));
        let reg = ProfileRegistry::from_profiles(m).unwrap();
        let set = reg.active_set(&["net".into()]).unwrap();
        assert_eq!(set.profile_for_kind("net:service").unwrap().id, "net");
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

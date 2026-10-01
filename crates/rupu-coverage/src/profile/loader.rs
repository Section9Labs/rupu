use crate::profile::{parse_profile, EngagementProfile, ProfileError};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error("profile include cycle at {0:?}")]
    IncludeCycle(String),
    #[error("unknown engagement profile {0:?} referenced as an include")]
    MissingInclude(String),
    #[error(transparent)]
    Profile(#[from] ProfileError),
}

/// Recursively merge `id`'s `includes` into one flattened profile.
pub fn expand_includes(
    all: &BTreeMap<String, EngagementProfile>,
    id: &str,
) -> Result<EngagementProfile, LoadError> {
    fn go(
        all: &BTreeMap<String, EngagementProfile>,
        id: &str,
        stack: &mut Vec<String>,
    ) -> Result<EngagementProfile, LoadError> {
        if stack.iter().any(|s| s == id) {
            return Err(LoadError::IncludeCycle(id.to_string()));
        }
        let base = all.get(id).ok_or_else(|| LoadError::MissingInclude(id.to_string()))?;
        let mut merged = base.clone();
        stack.push(id.to_string());
        for inc in &base.includes {
            let sub = go(all, inc, stack)?;
            merged.asset_kinds.extend(sub.asset_kinds);
            merged.evidence_blocks.extend(sub.evidence_blocks);
            merged.classification_systems.extend(sub.classification_systems);
            merged.completeness.extend(sub.completeness);
            merged.coverage.enumerates.extend(sub.coverage.enumerates);
            merged.coverage.depth_ladder.extend(sub.coverage.depth_ladder);
        }
        stack.pop();
        merged.includes.clear();
        dedup(&mut merged);
        Ok(merged)
    }
    go(all, id, &mut Vec::new())
}

fn dedup(p: &mut EngagementProfile) {
    p.evidence_blocks.sort();
    p.evidence_blocks.dedup();
    p.classification_systems.sort();
    p.classification_systems.dedup();
    // asset_kinds keep insertion order; drop later duplicates by id.
    let mut seen = std::collections::HashSet::new();
    p.asset_kinds.retain(|k| seen.insert(k.id.clone()));
    // completeness keep insertion order; drop later duplicates by id.
    let mut seen_completeness = std::collections::HashSet::new();
    p.completeness.retain(|c| seen_completeness.insert(c.id.clone()));
    // enumerates keep insertion order; drop later duplicates.
    let mut seen_enumerates = std::collections::HashSet::new();
    p.coverage.enumerates.retain(|e| seen_enumerates.insert(e.clone()));
    // depth_ladder keep insertion order; drop later duplicates.
    let mut seen_ladder = std::collections::HashSet::new();
    p.coverage.depth_ladder.retain(|d| seen_ladder.insert(d.clone()));
}

/// Discover `*.toml` profiles across dirs; later dirs override earlier by id.
pub fn discover(dirs: &[PathBuf]) -> BTreeMap<String, EngagementProfile> {
    let mut out = BTreeMap::new();
    for dir in dirs {
        let Ok(rd) = std::fs::read_dir(dir) else { continue };
        for entry in rd.flatten() {
            let path: &Path = &entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("toml") {
                continue;
            }
            let Ok(src) = std::fs::read_to_string(path) else { continue };
            if let Ok(p) = parse_profile(&src) {
                out.insert(p.id.clone(), p);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prof(id: &str, includes: &[&str], kind: &str) -> EngagementProfile {
        let mut p = crate::profile::parse_profile(&format!(
            "id=\"{id}\"\nname=\"{id}\"\n[[asset_kinds]]\nid=\"{kind}\"\nlabel=\"l\"\n[coverage]\n[bundle]\n"
        )).unwrap();
        p.includes = includes.iter().map(|s| s.to_string()).collect();
        p
    }

    #[test]
    fn composite_unions_included_kinds() {
        let mut all = BTreeMap::new();
        all.insert("network".into(), prof("network", &[], "service"));
        all.insert("web".into(), prof("web", &[], "route"));
        all.insert("pentest".into(), prof("pentest", &["network", "web"], "extra"));

        let p = expand_includes(&all, "pentest").unwrap();
        let ids: Vec<_> = p.asset_kinds.iter().map(|k| k.id.as_str()).collect();
        assert!(ids.contains(&"service") && ids.contains(&"route") && ids.contains(&"extra"));
        assert_eq!(p.id, "pentest");
    }

    #[test]
    fn cycles_and_missing_are_errors() {
        let mut all = BTreeMap::new();
        all.insert("a".into(), prof("a", &["b"], "ka"));
        all.insert("b".into(), prof("b", &["a"], "kb"));
        assert!(matches!(expand_includes(&all, "a"), Err(LoadError::IncludeCycle(_))));

        let mut m = BTreeMap::new();
        m.insert("x".into(), prof("x", &["missing"], "kx"));
        assert!(matches!(expand_includes(&m, "x"), Err(LoadError::MissingInclude(_))));
    }

    #[test]
    fn preserves_ladder_order() {
        let p = crate::profile::parse_profile(
            "id=\"ordered\"\nname=\"ordered\"\n[[asset_kinds]]\nid=\"test\"\nlabel=\"l\"\n[coverage]\ndepth_ladder=[\"located\",\"disassembled\",\"analyzed\"]\n[bundle]\n"
        ).unwrap();
        let mut all = BTreeMap::new();
        all.insert("ordered".into(), p);

        let result = expand_includes(&all, "ordered").unwrap();
        assert_eq!(result.coverage.depth_ladder, vec!["located", "disassembled", "analyzed"]);
    }
}

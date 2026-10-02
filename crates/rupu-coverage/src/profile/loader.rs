//! Profile loading: composite `includes` expansion, the include closure (for
//! routing), and filesystem discovery of overlay profiles. All fail-closed —
//! an unparseable discovered profile is an error, never a silent skip.

use super::package::{parse_profile, EngagementProfile};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Errors expanding or loading profiles.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LoadError {
    #[error("include cycle at profile `{0}`")]
    IncludeCycle(String),
    #[error("profile `{0}` is included but not defined")]
    MissingInclude(String),
}

/// Flatten a composite into the union of everything it includes: asset kinds,
/// evidence blocks, taxonomies, completeness checks, and the coverage lists.
/// (Routing does NOT use this — see [`include_closure`]; this is for callers
/// that want the merged declaration, e.g. a launcher bundle view.)
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
        let base = all
            .get(id)
            .ok_or_else(|| LoadError::MissingInclude(id.to_string()))?;
        let mut merged = base.clone();
        stack.push(id.to_string());
        for inc in &base.includes {
            let sub = go(all, inc, stack)?;
            merged.asset_kinds.extend(sub.asset_kinds);
            merged.evidence_blocks.extend(sub.evidence_blocks);
            merged
                .classification_systems
                .extend(sub.classification_systems);
            merged.completeness.extend(sub.completeness);
            merged.coverage.enumerates.extend(sub.coverage.enumerates);
            merged
                .coverage
                .depth_ladder
                .extend(sub.coverage.depth_ladder);
        }
        stack.pop();
        merged.includes.clear();
        dedup(&mut merged);
        Ok(merged)
    }
    go(all, id, &mut Vec::new())
}

/// Dedup a profile's list fields in place, preserving first-seen order.
pub(crate) fn dedup(p: &mut EngagementProfile) {
    dedup_strings(&mut p.evidence_blocks);
    dedup_strings(&mut p.classification_systems);
    dedup_strings(&mut p.coverage.enumerates);
    dedup_strings(&mut p.coverage.depth_ladder);
    let mut seen = std::collections::BTreeSet::new();
    p.asset_kinds.retain(|k| seen.insert(k.id.clone()));
    let mut seen_c = std::collections::BTreeSet::new();
    p.completeness.retain(|c| seen_c.insert(c.id.clone()));
}

fn dedup_strings(v: &mut Vec<String>) {
    let mut seen = std::collections::BTreeSet::new();
    v.retain(|s| seen.insert(s.clone()));
}

/// The profile ids `id` expands to for routing: `id` itself first, then every
/// profile it transitively includes (depth-first, in include order), each id
/// once. Fails on a cycle or a missing include.
pub fn include_closure(
    all: &BTreeMap<String, EngagementProfile>,
    id: &str,
) -> Result<Vec<String>, LoadError> {
    fn go(
        all: &BTreeMap<String, EngagementProfile>,
        id: &str,
        stack: &mut Vec<String>,
        out: &mut Vec<String>,
    ) -> Result<(), LoadError> {
        if stack.iter().any(|s| s == id) {
            return Err(LoadError::IncludeCycle(id.to_string()));
        }
        let base = all
            .get(id)
            .ok_or_else(|| LoadError::MissingInclude(id.to_string()))?;
        if out.iter().any(|s| s == id) {
            return Ok(());
        }
        out.push(id.to_string());
        stack.push(id.to_string());
        for inc in &base.includes {
            go(all, inc, stack, out)?;
        }
        stack.pop();
        Ok(())
    }
    let mut out = Vec::new();
    go(all, id, &mut Vec::new(), &mut out)?;
    Ok(out)
}

/// Discover `*.toml` profiles under each directory in order. Returns the parsed
/// profiles keyed by their `id`, and a list of human-readable errors. A missing
/// directory is skipped; an unreadable or unparseable file is an error (the
/// caller fails closed on a non-empty error list).
pub fn discover(dirs: &[PathBuf]) -> (BTreeMap<String, EngagementProfile>, Vec<String>) {
    let mut found = BTreeMap::new();
    let mut errors = Vec::new();
    for dir in dirs {
        discover_one(dir, &mut found, &mut errors);
    }
    (found, errors)
}

fn discover_one(
    dir: &Path,
    found: &mut BTreeMap<String, EngagementProfile>,
    errors: &mut Vec<String>,
) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
        Err(e) => {
            errors.push(format!("{}: {e}", dir.display()));
            return;
        }
    };
    let mut paths: Vec<PathBuf> = Vec::new();
    for entry in entries {
        match entry {
            Ok(e) => {
                let p = e.path();
                if p.extension().and_then(|x| x.to_str()) == Some("toml") {
                    paths.push(p);
                }
            }
            Err(e) => errors.push(format!("{}: {e}", dir.display())),
        }
    }
    paths.sort();
    for path in paths {
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                errors.push(format!("{}: {e}", path.display()));
                continue;
            }
        };
        match parse_profile(&text) {
            Ok(p) => {
                found.insert(p.id.clone(), p);
            }
            Err(e) => errors.push(format!("{}: {e}", path.display())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(id: &str, includes: &[&str], kind: &str) -> EngagementProfile {
        let inc = if includes.is_empty() {
            String::new()
        } else {
            format!(
                "includes = [{}]\n",
                includes
                    .iter()
                    .map(|i| format!("\"{i}\""))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        parse_profile(&format!(
            "id = \"{id}\"\nname = \"{id}\"\n{inc}[[asset_kinds]]\nid = \"{kind}\"\nlabel = \"l\"\n"
        ))
        .unwrap()
    }

    fn all() -> BTreeMap<String, EngagementProfile> {
        let mut m = BTreeMap::new();
        m.insert("network".into(), p("network", &[], "service"));
        m.insert("web".into(), p("web", &[], "route"));
        m.insert("pentest".into(), p("pentest", &["network", "web"], "x"));
        m
    }

    #[test]
    fn expand_unions_included_kinds() {
        let merged = expand_includes(&all(), "pentest").unwrap();
        let kinds: Vec<&str> = merged.asset_kinds.iter().map(|k| k.id.as_str()).collect();
        assert!(kinds.contains(&"service") && kinds.contains(&"route") && kinds.contains(&"x"));
        assert!(merged.includes.is_empty());
    }

    #[test]
    fn closure_lists_self_then_members_once() {
        let c = include_closure(&all(), "pentest").unwrap();
        assert_eq!(c, ["pentest", "network", "web"]);
    }

    #[test]
    fn cycle_and_missing_are_errors() {
        let mut m = BTreeMap::new();
        m.insert("a".into(), p("a", &["b"], "ka"));
        m.insert("b".into(), p("b", &["a"], "kb"));
        assert!(matches!(
            include_closure(&m, "a"),
            Err(LoadError::IncludeCycle(_))
        ));
        let mut m2 = BTreeMap::new();
        m2.insert("a".into(), p("a", &["ghost"], "ka"));
        assert!(matches!(
            include_closure(&m2, "a"),
            Err(LoadError::MissingInclude(_))
        ));
    }

    #[test]
    fn discover_parses_tomls_and_fails_closed_on_junk() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("good.toml"),
            "id = \"good\"\nname = \"g\"\n",
        )
        .unwrap();
        std::fs::write(tmp.path().join("bad.toml"), "id = \"b\"\nbogus = 1\n").unwrap();
        let (found, errors) = discover(&[tmp.path().to_path_buf()]);
        assert!(found.contains_key("good"));
        assert_eq!(errors.len(), 1, "the junk file must surface an error");
        // a missing dir is skipped silently
        let (f2, e2) = discover(&[tmp.path().join("nope")]);
        assert!(f2.is_empty() && e2.is_empty());
    }
}

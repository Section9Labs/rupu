//! The `skills:` grant grammar (spec §2.1). `*` is the global layer only;
//! a project skill needs its exact name or `project:*`; a bare name in
//! both layers is an error until qualified.

use crate::catalog::{CatalogEntry, SkillCatalog};
use crate::name::{NameError, SkillName};
use crate::skill::{Layer, Skill, SkillError};
use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillGrant {
    Name(SkillName),
    Qualified(Layer, SkillName),
    AllGlobal,
    AllProject,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GrantParseError {
    #[error("entry {entry:?}: {source}")]
    Name { entry: String, source: NameError },
    #[error("entry {entry:?}: unknown layer {prefix:?} (use global: or project:)")]
    UnknownLayer { entry: String, prefix: String },
}

impl FromStr for SkillGrant {
    type Err = GrantParseError;

    fn from_str(entry: &str) -> Result<Self, GrantParseError> {
        let name = |s: &str| {
            s.parse::<SkillName>()
                .map_err(|source| GrantParseError::Name {
                    entry: entry.to_owned(),
                    source,
                })
        };
        if entry == "*" {
            return Ok(SkillGrant::AllGlobal);
        }
        let Some((prefix, rest)) = entry.split_once(':') else {
            return Ok(SkillGrant::Name(name(entry)?));
        };
        let layer = Layer::parse(prefix).ok_or_else(|| GrantParseError::UnknownLayer {
            entry: entry.to_owned(),
            prefix: prefix.to_owned(),
        })?;
        Ok(match (layer, rest) {
            (Layer::Global, "*") => SkillGrant::AllGlobal,
            (Layer::Project, "*") => SkillGrant::AllProject,
            (layer, rest) => SkillGrant::Qualified(layer, name(rest)?),
        })
    }
}

pub fn parse_grants(entries: &[String]) -> Result<Vec<SkillGrant>, GrantParseError> {
    entries.iter().map(|e| e.parse()).collect()
}

/// A catalog entry's load error, shared with the catalog that cached it.
/// Two are equal when they are the same entry's failure.
#[derive(Debug, Clone)]
pub struct EntryError(pub Arc<SkillError>);

impl PartialEq for EntryError {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for EntryError {}

impl fmt::Display for EntryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveError {
    Unknown {
        entry: String,
        suggestion: Option<String>,
    },
    Invalid {
        name: String,
        layer: Layer,
        error: EntryError,
    },
    Ambiguous {
        name: String,
    },
    BothLayers {
        name: String,
    },
}

impl fmt::Display for ResolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ResolveError::Unknown {
                entry,
                suggestion: Some(s),
            } => {
                write!(f, "unknown skill {entry:?} (did you mean {s}?)")
            }
            ResolveError::Unknown {
                entry,
                suggestion: None,
            } => write!(f, "unknown skill {entry:?}"),
            ResolveError::Invalid { name, layer, error } => {
                write!(f, "skill {name:?} ({}) is invalid: {error}", layer.as_str())
            }
            ResolveError::Ambiguous { name } => write!(
                f,
                "skill {name:?} is installed in both the global and the project layer; \
                 grant global:{name} or project:{name}"
            ),
            ResolveError::BothLayers { name } => write!(
                f,
                "skill {name:?} is granted from both the global and the project layer; \
                 grant only one of global:{name} or project:{name}"
            ),
        }
    }
}

impl std::error::Error for ResolveError {}

pub fn resolve_grants(
    grants: &[SkillGrant],
    catalog: &SkillCatalog,
) -> Result<Vec<Arc<Skill>>, Vec<ResolveError>> {
    let mut picked: BTreeMap<(String, Layer), Arc<Skill>> = BTreeMap::new();
    let mut errors = Vec::new();
    let mut take = |e: &CatalogEntry| match e.load() {
        Ok(s) => {
            picked.insert((e.dir_name.clone(), e.layer), s.clone());
            None
        }
        Err(err) => Some(ResolveError::Invalid {
            name: e.dir_name.clone(),
            layer: e.layer,
            error: EntryError(err.clone()),
        }),
    };
    for g in grants {
        let err = match g {
            SkillGrant::AllGlobal | SkillGrant::AllProject => {
                let layer = if *g == SkillGrant::AllGlobal {
                    Layer::Global
                } else {
                    Layer::Project
                };
                for e in catalog
                    .entries()
                    .iter()
                    .filter(|e| e.layer == layer && e.is_valid())
                {
                    take(e);
                }
                None
            }
            SkillGrant::Qualified(layer, name) => match catalog.get(*layer, name.as_str()) {
                Some(e) => take(e),
                None => Some(unknown(
                    &format!("{}:{name}", layer.as_str()),
                    Some(*layer),
                    name.as_str(),
                    catalog,
                    CatalogEntry::is_valid,
                )),
            },
            SkillGrant::Name(name) => {
                let found: Vec<&CatalogEntry> = catalog.named(name.as_str()).collect();
                match found.as_slice() {
                    [] => Some(unknown(
                        name.as_str(),
                        None,
                        name.as_str(),
                        catalog,
                        CatalogEntry::is_valid,
                    )),
                    [one] => take(one),
                    _ => Some(ResolveError::Ambiguous {
                        name: name.to_string(),
                    }),
                }
            }
        };
        errors.extend(err);
    }
    errors.extend(
        picked
            .keys()
            .filter(|(name, layer)| {
                *layer == Layer::Global && picked.contains_key(&(name.clone(), Layer::Project))
            })
            .map(|(name, _)| ResolveError::BothLayers { name: name.clone() }),
    );
    let mut unique: Vec<ResolveError> = Vec::with_capacity(errors.len());
    for e in errors {
        if !unique.contains(&e) {
            unique.push(e);
        }
    }
    if unique.is_empty() {
        Ok(picked.into_values().collect())
    } else {
        Err(unique)
    }
}

/// Whether every entry names an installed skill, from the catalog's
/// listing alone: no skill is loaded or hashed, so this is what an agent
/// load checks. An unknown name (with a did-you-mean) or a bare name in both
/// layers fails; whether a named skill is valid, and whether the set binds a
/// name twice, is `resolve_run_skills`'s to say.
pub fn check_grant_names(
    entries: &[String],
    catalog: &SkillCatalog,
) -> Result<(), crate::run::RunSkillsError> {
    let listed = |_: &CatalogEntry| true;
    let errors: Vec<ResolveError> = parse_grants(entries)?
        .iter()
        .filter_map(|g| match g {
            SkillGrant::AllGlobal | SkillGrant::AllProject => None,
            SkillGrant::Qualified(layer, name) => {
                catalog.get(*layer, name.as_str()).is_none().then(|| {
                    unknown(
                        &format!("{}:{name}", layer.as_str()),
                        Some(*layer),
                        name.as_str(),
                        catalog,
                        listed,
                    )
                })
            }
            SkillGrant::Name(name) => match catalog.named(name.as_str()).count() {
                0 => Some(unknown(name.as_str(), None, name.as_str(), catalog, listed)),
                1 => None,
                _ => Some(ResolveError::Ambiguous {
                    name: name.to_string(),
                }),
            },
        })
        .collect();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(crate::run::RunSkillsError::Resolve(errors))
    }
}

/// Suggests the closest skill `candidate` accepts. A qualified grant is answered in the
/// layer the match lives in; a bare grant is answered qualified when the
/// match is in both layers, since the bare name would be ambiguous.
fn unknown(
    entry: &str,
    layer: Option<Layer>,
    name: &str,
    catalog: &SkillCatalog,
    candidate: impl Fn(&CatalogEntry) -> bool,
) -> ResolveError {
    let len = name.chars().count();
    let limit = (len / 3).max(2);
    let suggestion = catalog
        .entries()
        .iter()
        .filter(|e| candidate(e))
        .map(|e| {
            let d = edit_distance(name, &e.dir_name);
            (d, Some(e.layer) != layer, &e.dir_name, e.layer)
        })
        .filter(|(d, ..)| *d <= limit && *d < len)
        .min()
        .map(|(_, _, found, found_layer)| {
            let in_both = catalog.named(found).filter(|e| candidate(e)).count() > 1;
            match layer {
                Some(_) => format!("{}:{found}", found_layer.as_str()),
                None if in_both => format!("global:{found}"),
                None => found.clone(),
            }
        });
    ResolveError::Unknown {
        entry: entry.to_owned(),
        suggestion,
    }
}

fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let sub = prev[j] + usize::from(ca != *cb);
            cur.push(sub.min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}

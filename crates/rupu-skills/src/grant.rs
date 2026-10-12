//! The `skills:` grant grammar (spec §2.1). `*` is the global layer only;
//! a project skill needs its exact name or `project:*`; a bare name in
//! both layers is an error until qualified.

use crate::catalog::{CatalogEntry, SkillCatalog};
use crate::name::{NameError, SkillName};
use crate::skill::{Layer, Skill};
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
    #[error("skills: entry {entry:?}: {source}")]
    Name { entry: String, source: NameError },
    #[error("skills: entry {entry:?}: unknown layer {prefix:?} (use global: or project:)")]
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveError {
    Unknown {
        entry: String,
        suggestion: Option<String>,
    },
    Invalid {
        entry: String,
        layer: Layer,
        error: String,
    },
    Ambiguous {
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
            ResolveError::Invalid {
                entry,
                layer,
                error,
            } => {
                write!(
                    f,
                    "skill {entry:?} ({}) is invalid: {error}",
                    layer.as_str()
                )
            }
            ResolveError::Ambiguous { name } => write!(
                f,
                "skill {name:?} is installed in both the global and the project layer; \
                 grant global:{name} or project:{name}"
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
    let mut take = |e: &CatalogEntry, entry: &str, errors: &mut Vec<ResolveError>| match &e.skill {
        Ok(s) => {
            picked.insert((e.dir_name.clone(), e.layer), s.clone());
        }
        Err(err) => errors.push(ResolveError::Invalid {
            entry: entry.to_owned(),
            layer: e.layer,
            error: err.to_string(),
        }),
    };
    for g in grants {
        match g {
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
                    take(e, &e.dir_name, &mut errors);
                }
            }
            SkillGrant::Qualified(layer, name) => match catalog.get(*layer, name.as_str()) {
                Some(e) => take(e, &format!("{}:{name}", layer.as_str()), &mut errors),
                None => errors.push(unknown(
                    &format!("{}:{name}", layer.as_str()),
                    name.as_str(),
                    catalog,
                )),
            },
            SkillGrant::Name(name) => {
                let found: Vec<&CatalogEntry> = catalog.named(name.as_str()).collect();
                match found.as_slice() {
                    [] => errors.push(unknown(name.as_str(), name.as_str(), catalog)),
                    [one] => take(one, name.as_str(), &mut errors),
                    _ => errors.push(ResolveError::Ambiguous {
                        name: name.to_string(),
                    }),
                }
            }
        }
    }
    if errors.is_empty() {
        Ok(picked.into_values().collect())
    } else {
        Err(errors)
    }
}

fn unknown(entry: &str, name: &str, catalog: &SkillCatalog) -> ResolveError {
    let limit = (name.len() / 3).max(2);
    let suggestion = catalog
        .entries()
        .iter()
        .map(|e| (edit_distance(name, &e.dir_name), &e.dir_name))
        .filter(|(d, _)| *d <= limit)
        .min()
        .map(|(_, n)| n.clone());
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

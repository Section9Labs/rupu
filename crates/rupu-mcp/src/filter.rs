//! Which tools a server lists (`--tools`).
//!
//! Grammar, per entry: `*` (all), `core.*` (the catalog's core tools),
//! `<ns>.*` (every name whose first dotted segment is `<ns>`), an exact
//! name, or a catalog legacy alias (resolved to its canonical name). No
//! entries means everything.

use std::collections::BTreeSet;

use rupu_tools::{AliasScope, ToolCatalog};

use crate::error::ServeError;

/// A selection over tool names.
#[derive(Debug, Clone, Default)]
pub struct ToolFilter {
    entries: Vec<String>,
}

impl ToolFilter {
    /// Keeps every tool.
    pub fn all() -> Self {
        Self::default()
    }

    /// A filter from `--tools` entries (already split on commas by clap).
    /// Empty = everything.
    pub fn parse(entries: &[String]) -> Self {
        Self {
            entries: entries
                .iter()
                .map(|e| e.trim().to_string())
                .filter(|e| !e.is_empty())
                .collect(),
        }
    }

    /// The subset of `names` kept. The first entry that matches none of
    /// them is `ServeError::Filter`.
    pub fn apply(&self, names: &[String]) -> Result<BTreeSet<String>, ServeError> {
        if self.entries.is_empty() {
            return Ok(names.iter().cloned().collect());
        }
        let catalog = ToolCatalog::builtin();
        let mut kept = BTreeSet::new();
        for entry in &self.entries {
            let mut matched = false;
            if entry == "*" {
                matched = !names.is_empty();
                kept.extend(names.iter().cloned());
            } else if entry == "core.*" {
                for n in names {
                    if ToolCatalog::all()
                        .iter()
                        .any(|d| d.is_core() && d.name == n)
                    {
                        matched = true;
                        kept.insert(n.clone());
                    }
                }
            } else if let Some(ns) = entry.strip_suffix(".*") {
                for n in names {
                    if n.split('.').next() == Some(ns) && n.contains('.') {
                        matched = true;
                        kept.insert(n.clone());
                    }
                }
            } else {
                let canonical = catalog
                    .resolve_name(entry, AliasScope::Everywhere)
                    .map(|d| d.name)
                    .unwrap_or(entry.as_str());
                for n in names {
                    if n == entry || n == canonical {
                        matched = true;
                        kept.insert(n.clone());
                    }
                }
            }
            if !matched {
                let namespaces: BTreeSet<&str> = names
                    .iter()
                    .filter_map(|n| n.split_once('.').map(|(ns, _)| ns))
                    .collect();
                return Err(ServeError::Filter(format!(
                    "--tools: `{entry}` matches no tool; namespaces here: {}",
                    namespaces.into_iter().collect::<Vec<_>>().join(", ")
                )));
            }
        }
        Ok(kept)
    }
}

//! Which tools a server lists (`--tools`).
//!
//! Grammar, per entry: `*` (all), `core.*` (the catalog's core tools),
//! `<ns>.*` (every name whose first dotted segment is `<ns>`), an exact
//! name, or a catalog legacy alias (resolved to its canonical name). No
//! entries means everything.

use std::collections::BTreeSet;

use rupu_tools::{edit_distance, AliasScope, ToolCatalog};

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
                matched = true;
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
                let hint = suggest(entry, names, &namespaces)
                    .map(|s| format!("; did you mean `{s}`?"))
                    .unwrap_or_default();
                return Err(ServeError::Filter(format!(
                    "--tools: `{entry}` matches no tool; namespaces here: {}{hint}",
                    namespaces.into_iter().collect::<Vec<_>>().join(", ")
                )));
            }
        }
        Ok(kept)
    }
}

/// The served name, alias of a served tool, `<ns>.*` or `core.*` closest to
/// `entry` (case-insensitive, within edit distance 2). A tie goes to the
/// first in sorted order.
fn suggest(entry: &str, names: &[String], namespaces: &BTreeSet<&str>) -> Option<String> {
    let served: BTreeSet<&str> = names.iter().map(String::as_str).collect();
    let mut candidates: BTreeSet<String> = served.iter().map(|n| n.to_string()).collect();
    candidates.extend(namespaces.iter().map(|ns| format!("{ns}.*")));
    candidates.insert("core.*".to_string());
    for d in ToolCatalog::all() {
        if served.contains(d.name) {
            candidates.extend(d.aliases.iter().map(|a| a.name.to_string()));
        }
    }
    let typed = entry.to_ascii_lowercase();
    let mut best: Option<(usize, &String)> = None;
    for c in &candidates {
        let dist = edit_distance(&typed, &c.to_ascii_lowercase());
        if dist <= 2 && best.is_none_or(|(b, _)| dist < b) {
            best = Some((dist, c));
        }
    }
    best.map(|(_, c)| c.clone())
}

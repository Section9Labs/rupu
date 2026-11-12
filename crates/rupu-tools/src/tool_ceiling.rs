//! An exact upper bound over a resolved grant, including ambient and owned tools.
//! It never grants a tool. Session callers persist the creation-time list.
use crate::{GrantError, ResolvedGrant, ToolCatalog};
use std::collections::BTreeSet;

pub const FEATURE: &str = "session.tool_ceiling.v1";
pub const MAX_TOOLS: usize = 256;

/// Validate exact canonical names against a captured catalog and separately
/// frozen remote identities. Aliases, patterns, duplicates and unknowns refuse.
pub fn validate_names(
    names: &[String],
    catalog: &ToolCatalog,
    remote_names: impl IntoIterator<Item = String>,
) -> Result<(), GrantError> {
    let remote: BTreeSet<String> = remote_names.into_iter().collect();
    let mut seen = BTreeSet::new();
    if names.len() > MAX_TOOLS {
        return Err(GrantError::InvalidToolCeiling);
    }
    for name in names {
        if name.is_empty()
            || name.len() > 256
            || name.contains('*')
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
            || !seen.insert(name)
            || (catalog.definition(name).is_none() && !remote.contains(name))
        {
            return Err(GrantError::InvalidToolCeiling);
        }
    }
    Ok(())
}

/// Last grant transformation, after declarations, origin, ambient and MCP
/// assembly. Registry/provider schemas are both built solely from these entries.
pub fn apply(grant: &mut ResolvedGrant, ceiling: Option<&[String]>) -> Result<(), GrantError> {
    apply_with_frozen_remote(grant, ceiling, std::iter::empty())
}

/// Creation-frozen remote identities remain valid ceiling names even when later
/// settings deselect them. They validate names only and never supply entries.
pub fn apply_with_frozen_remote(
    grant: &mut ResolvedGrant,
    ceiling: Option<&[String]>,
    frozen_remote_names: impl IntoIterator<Item = String>,
) -> Result<(), GrantError> {
    let Some(names) = ceiling else {
        return Ok(());
    };
    validate_names(names, &grant.catalog, frozen_remote_names)?;
    let allowed: BTreeSet<&str> = names.iter().map(String::as_str).collect();
    let mut excluded = Vec::new();
    grant.entries.retain(|name, _| {
        let keep = allowed.contains(name.as_str());
        if !keep {
            excluded.push(name.clone());
        }
        keep
    });
    for entries in [&mut grant.unavailable, &mut grant.skipped] {
        entries.retain(|entry| {
            let keep = allowed.contains(entry.tool.as_str());
            if !keep {
                excluded.push(entry.tool.clone());
            }
            keep
        });
    }
    grant.narrowed.extend(excluded);
    grant.narrowed.sort();
    grant.narrowed.dedup();
    grant.tool_ceiling = Some(names.to_vec());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        grant::{GrantInputs, ServiceSet},
        AmbientGrant,
    };
    fn resolved() -> ResolvedGrant {
        let catalog = ToolCatalog::builtin();
        let mut available = ServiceSet::new();
        for d in catalog.definitions() {
            for need in &d.needs {
                available.insert(*need);
            }
        }
        catalog
            .resolve_grant(GrantInputs {
                declared: Some(&["read_file".into()]),
                step_actions: &[],
                connector_cap: None,
                ambient: &[
                    AmbientGrant::concerns(),
                    AmbientGrant::skills(),
                    AmbientGrant::flow_lead(),
                    AmbientGrant::messaging_default(),
                ],
                available: &available,
                alias_scope: Default::default(),
            })
            .unwrap()
    }
    #[test]
    fn immutable_ceiling_intersects_all_ambient_sources_without_adding_grants() {
        let mut grant = resolved();
        assert!(
            grant.offers("findings.report")
                && grant.offers("board.post")
                && grant.offers("skills.load")
        );
        let descriptor = grant.entries["skills.load"].descriptor.clone();
        let reasons = grant.entries["skills.load"].reasons.clone();
        apply(
            &mut grant,
            Some(&["skills.load".into(), "write_file".into()]),
        )
        .unwrap();
        assert_eq!(
            grant.entries.keys().cloned().collect::<Vec<_>>(),
            ["skills.load"]
        );
        assert!(std::sync::Arc::ptr_eq(
            &descriptor,
            &grant.entries["skills.load"].descriptor
        ));
        assert_eq!(grant.entries["skills.load"].reasons, reasons);
        assert!(grant.narrowed.iter().any(|n| n == "findings.report"));
    }
    #[test]
    fn empty_ceiling_denies_even_ambient_and_none_is_legacy_identical() {
        let mut grant = resolved();
        let before = grant.entries.keys().cloned().collect::<Vec<_>>();
        apply(&mut grant, None).unwrap();
        assert_eq!(before, grant.entries.keys().cloned().collect::<Vec<_>>());
        assert!(grant.tool_ceiling.is_none());
        apply(&mut grant, Some(&[])).unwrap();
        assert!(
            grant.entries.is_empty() && grant.unavailable.is_empty() && grant.skipped.is_empty()
        );
        assert_eq!(grant.tool_ceiling, Some(vec![]));
    }
    #[test]
    fn deselecting_frozen_mcp_tool_keeps_ceiling_without_regranting_it() {
        let mut grant = resolved();
        let ceiling = vec!["mcp.frozen_fixture".into(), "skills.load".into()];
        assert!(apply(&mut grant, Some(&ceiling)).is_err());
        apply_with_frozen_remote(&mut grant, Some(&ceiling), ["mcp.frozen_fixture".into()])
            .unwrap();
        assert_eq!(
            grant.entries.keys().cloned().collect::<Vec<_>>(),
            ["skills.load"]
        );
        assert!(!grant.offers("mcp.frozen_fixture"));
        assert_eq!(grant.tool_ceiling, Some(ceiling));
    }
    #[test]
    fn ceiling_refuses_aliases_patterns_unknown_duplicates_and_oversize() {
        for names in [
            vec!["skill_load".into()],
            vec!["skills.*".into()],
            vec!["*".into()],
            vec!["unknown".into()],
            vec![" skills.load".into()],
            vec!["skills.load".into(); 2],
            vec!["read_file".into(); 257],
        ] {
            let mut grant = resolved();
            let before = grant.entries.len();
            assert!(apply(&mut grant, Some(&names)).is_err());
            assert_eq!(grant.entries.len(), before, "validation precedes mutation");
        }
    }
}

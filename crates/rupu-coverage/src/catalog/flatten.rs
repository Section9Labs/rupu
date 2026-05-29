use crate::catalog::builtin::resolve_builtin;
use crate::catalog::types::{Concern, ConcernsBlock, ConcernsEntry, FlatCatalog, Template};
use std::collections::BTreeMap;

#[derive(Debug, thiserror::Error)]
pub enum FlattenError {
    #[error("unknown template `{0}`")]
    UnknownTemplate(String),
    #[error("template `{0}` failed to parse: {1}")]
    TemplateParse(String, String),
    #[error("duplicate concern_id `{id}` from `{first}` and `{second}` — declare an explicit override to resolve")]
    DuplicateId {
        id: String,
        first: String,
        second: String,
    },
    #[error("override targets unknown concern_id `{id}` in include `{template}`")]
    OverrideUnknownId { template: String, id: String },
}

pub fn flatten(block: &ConcernsBlock) -> Result<FlatCatalog, FlattenError> {
    flatten_with_resolver(block, &|name| {
        resolve_builtin(name)
            .ok_or_else(|| FlattenError::UnknownTemplate(name.to_string()))?
            .map_err(|e| FlattenError::TemplateParse(name.to_string(), e.to_string()))
    })
}

/// Lower-level entry point used by tests and by callers (such as the agent
/// runner) that need to resolve templates beyond the builtin set
/// (e.g. project-level templates under .rupu/concerns/).
pub fn flatten_with_resolver<F>(
    block: &ConcernsBlock,
    resolve: &F,
) -> Result<FlatCatalog, FlattenError>
where
    F: Fn(&str) -> Result<Template, FlattenError>,
{
    // Pass 1: collect inline concerns first so they win on duplicate ids.
    let mut by_id: BTreeMap<String, Concern> = BTreeMap::new();
    let mut sources: BTreeMap<String, String> = BTreeMap::new();
    for entry in &block.entries {
        if let ConcernsEntry::Inline(concern) = entry {
            by_id.insert(concern.id.clone(), concern.clone());
            sources.insert(concern.id.clone(), "inline".to_string());
        }
    }

    // Pass 2: resolve includes, recursing if a template `includes:` other templates.
    for entry in &block.entries {
        let ConcernsEntry::Include(directive) = entry else {
            continue;
        };
        let template = resolve(&directive.include)?;
        let mut template_concerns = template.concerns.clone();

        // Recurse into nested includes (composite templates like
        // web-security-default that list `includes: [...]`).
        for nested_name in &template.includes {
            let nested = resolve(nested_name)?;
            template_concerns.extend(nested.concerns);
        }

        // Apply overrides — must target a concern that exists in the
        // resolved template (after nested includes).
        let template_ids: std::collections::HashSet<&str> =
            template_concerns.iter().map(|c| c.id.as_str()).collect();
        for over in &directive.overrides {
            if !template_ids.contains(over.id.as_str()) {
                return Err(FlattenError::OverrideUnknownId {
                    template: directive.include.clone(),
                    id: over.id.clone(),
                });
            }
        }

        for mut concern in template_concerns {
            // Inline wins.
            if by_id.contains_key(&concern.id)
                && sources.get(&concern.id).map(String::as_str) == Some("inline")
            {
                continue;
            }
            // Apply override if present.
            if let Some(over) = directive.overrides.iter().find(|o| o.id == concern.id) {
                if let Some(s) = over.severity {
                    concern.severity = s;
                }
                if let Some(g) = over.applicable_globs.clone() {
                    concern.applicable_globs = g;
                }
                if let Some(m) = over.min_strength {
                    concern.min_strength = m;
                }
                if let Some(r) = over.references.clone() {
                    concern.references = r;
                }
                if let Some(t) = over.tags.clone() {
                    concern.tags = t;
                }
                if let Some(d) = over.description.clone() {
                    concern.description = d;
                }
            }

            // Duplicate-id detection across includes.
            if let Some(existing_source) = sources.get(&concern.id) {
                if existing_source != "inline" && existing_source != &directive.include {
                    return Err(FlattenError::DuplicateId {
                        id: concern.id.clone(),
                        first: existing_source.clone(),
                        second: directive.include.clone(),
                    });
                }
            }

            by_id.insert(concern.id.clone(), concern.clone());
            sources
                .entry(concern.id.clone())
                .or_insert_with(|| directive.include.clone());
        }
    }

    Ok(FlatCatalog {
        concerns: by_id.into_values().collect(),
        sources,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::types::{ConcernOverride, IncludeDirective, Severity, TouchStrength};

    fn inline_concern(id: &str) -> Concern {
        Concern {
            id: id.to_string(),
            name: id.to_string(),
            description: "test".to_string(),
            severity: Severity::Low,
            applicable_globs: vec!["**".to_string()],
            min_strength: TouchStrength::Read,
            references: vec![],
            tags: vec![],
        }
    }

    #[test]
    fn flatten_single_include_pulls_template_concerns() {
        let block = ConcernsBlock {
            entries: vec![ConcernsEntry::Include(IncludeDirective {
                include: "stride".to_string(),
                overrides: vec![],
                mode: crate::catalog::types::CatalogMode::Auto,
                filter: None,
            })],
        };
        let cat = flatten(&block).unwrap();
        assert_eq!(cat.concerns.len(), 6);
        assert!(cat.concerns.iter().any(|c| c.id == "stride:spoofing"));
    }

    #[test]
    fn inline_concern_wins_over_include() {
        let mut custom = inline_concern("stride:spoofing");
        custom.description = "OVERRIDDEN".to_string();
        let block = ConcernsBlock {
            entries: vec![
                ConcernsEntry::Include(IncludeDirective {
                    include: "stride".to_string(),
                    overrides: vec![],
                    mode: crate::catalog::types::CatalogMode::Auto,
                    filter: None,
                }),
                ConcernsEntry::Inline(custom),
            ],
        };
        let cat = flatten(&block).unwrap();
        let spoofing = cat
            .concerns
            .iter()
            .find(|c| c.id == "stride:spoofing")
            .unwrap();
        assert_eq!(spoofing.description, "OVERRIDDEN");
        assert_eq!(cat.sources["stride:spoofing"], "inline");
    }

    #[test]
    fn override_directive_patches_single_field() {
        let block = ConcernsBlock {
            entries: vec![ConcernsEntry::Include(IncludeDirective {
                include: "stride".to_string(),
                overrides: vec![ConcernOverride {
                    id: "stride:spoofing".to_string(),
                    severity: Some(Severity::Critical),
                    ..Default::default()
                }],
                mode: crate::catalog::types::CatalogMode::Auto,
                filter: None,
            })],
        };
        let cat = flatten(&block).unwrap();
        let spoofing = cat
            .concerns
            .iter()
            .find(|c| c.id == "stride:spoofing")
            .unwrap();
        assert_eq!(spoofing.severity, Severity::Critical);
    }

    #[test]
    fn unknown_template_errors() {
        let block = ConcernsBlock {
            entries: vec![ConcernsEntry::Include(IncludeDirective {
                include: "not-a-real-template".to_string(),
                overrides: vec![],
                mode: crate::catalog::types::CatalogMode::Auto,
                filter: None,
            })],
        };
        let err = flatten(&block).unwrap_err();
        assert!(matches!(err, FlattenError::UnknownTemplate(_)));
    }

    #[test]
    fn composite_template_resolves_nested_includes() {
        let block = ConcernsBlock {
            entries: vec![ConcernsEntry::Include(IncludeDirective {
                include: "web-security-default".to_string(),
                overrides: vec![],
                mode: crate::catalog::types::CatalogMode::Auto,
                filter: None,
            })],
        };
        let cat = flatten(&block).unwrap();
        // owasp-top10-2021 (10) + cwe-top25-2023 (25) + secrets-in-source (1) = 36
        // assuming no id collisions between the three templates.
        assert!(cat.concerns.len() >= 30);
        assert!(cat.concerns.iter().any(|c| c.id.starts_with("owasp-top10-2021:")));
        assert!(cat.concerns.iter().any(|c| c.id.starts_with("cwe-top25-2023:")));
        assert!(cat.concerns.iter().any(|c| c.id == "secrets-in-source"));
    }
}

use crate::asset::{default_label, read_asset_graph, upsert_asset, Asset, AssetId, Locator};
use crate::ledger::events::Attribution;
use crate::ledger::paths::CoveragePaths;
use serde::{Deserialize, Serialize};

/// Register an asset and/or set how deeply it has been covered.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetMarkInput {
    /// Profile-namespaced kind, e.g. `binary:function`.
    pub kind: String,
    pub locator: Locator,
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
    /// A rung of the owning profile's `coverage.depth_ladder`.
    pub depth: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetMarkOutput {
    pub id: String,
    pub kind: String,
    /// The EFFECTIVE depth now stored: the requested rung, or, when that is
    /// shallower than what the asset already had, the deeper rung that was
    /// kept (depth is monotonic, so a shallower mark is clamped, not applied).
    pub depth: String,
}

#[derive(Debug, thiserror::Error)]
pub enum AssetMarkError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("asset_mark needs an active engagement profile (this run has none, so there are no asset kinds or depth ladders to mark against)")]
    NoEngagement,
    #[error("asset kind `{kind}` belongs to no active engagement profile (active: {active}); mark an asset whose kind one of them declares")]
    KindNotActive { kind: String, active: String },
    #[error("engagement profile `{profile}` does not declare asset kind `{kind}` (it declares: {declared})")]
    KindNotDeclared {
        profile: String,
        kind: String,
        declared: String,
    },
    #[error("`{depth}` is not a depth for `{kind}`: engagement profile `{profile}` has the depth ladder [{ladder}]")]
    UnknownDepth {
        profile: String,
        kind: String,
        depth: String,
        ladder: String,
    },
    #[error("`locator` is empty: an asset is identified by its locator, so name at least one coordinate")]
    EmptyLocator,
}

/// Register an asset and advance its coverage depth.
///
/// Fail-closed on the engagement: `kind` must be owned by an active profile
/// AND declared by it (`ActiveSet::profile_for_kind` routes by namespace
/// alone, so `binary:nope` would otherwise reach `binary`), and `depth` must be
/// a rung of THAT profile's `coverage.depth_ladder`.
///
/// The write is an upsert, merged over the existing record: an agent that
/// marks an asset it only knows by locator must not erase the parent, label or
/// attributes an earlier pass recorded. Only `depth` is set, and a parent or
/// label named here overrides.
///
/// Depth is MONOTONIC: it advances along the owning profile's ladder and never
/// regresses. When the requested rung is shallower than the one the asset
/// already has, the deeper existing rung is kept (the request is clamped) and
/// the returned [`AssetMarkOutput::depth`] reports that EFFECTIVE rung, so the
/// caller can see the clamp rather than believe it walked the asset backward.
/// An existing depth that is no longer on the ladder (a profile edited between
/// runs) cannot be ordered, so the requested rung wins, with a warning.
pub fn asset_mark(
    paths: &CoveragePaths,
    attribution: Attribution,
    input: AssetMarkInput,
    opts: &crate::report::FindingWriteOptions,
) -> Result<AssetMarkOutput, AssetMarkError> {
    let engagement = opts
        .engagement
        .as_deref()
        .ok_or(AssetMarkError::NoEngagement)?;
    let profile =
        engagement
            .profile_for_kind(&input.kind)
            .ok_or_else(|| AssetMarkError::KindNotActive {
                kind: input.kind.clone(),
                active: engagement.ids().join(", "),
            })?;
    let kind_def = profile
        .asset_kinds
        .iter()
        .find(|k| k.id == input.kind)
        .ok_or_else(|| AssetMarkError::KindNotDeclared {
            profile: profile.id.clone(),
            kind: input.kind.clone(),
            declared: profile
                .asset_kinds
                .iter()
                .map(|k| k.id.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        })?;
    if !profile.coverage.depth_ladder.contains(&input.depth) {
        return Err(AssetMarkError::UnknownDepth {
            profile: profile.id.clone(),
            kind: input.kind,
            depth: input.depth,
            ladder: profile.coverage.depth_ladder.join(", "),
        });
    }
    if input.locator.0.is_empty() {
        return Err(AssetMarkError::EmptyLocator);
    }

    let label_given = input.label.is_some();
    let label = input
        .label
        .unwrap_or_else(|| default_label(&input.kind, Some(&kind_def.label), &input.locator));
    let mut asset = Asset::new(input.kind, input.locator, label);
    asset.parent = input.parent.map(AssetId);
    let depth = {
        let graph = read_asset_graph(paths);
        let existing = graph.get(&asset.id).and_then(|a| a.depth.as_deref());
        monotonic_depth(
            &profile.coverage.depth_ladder,
            existing,
            input.depth,
            &asset,
        )
    };
    let out = AssetMarkOutput {
        id: asset.id.0.clone(),
        kind: asset.kind.clone(),
        depth: depth.clone(),
    };
    upsert_asset(paths, asset, label_given, Some(depth), &attribution)?;
    Ok(out)
}

/// The depth to store: the deeper of `existing` and `requested` by their
/// position in `ladder` (shallowest to deepest). `requested` is already known
/// to be on the ladder; an `existing` that is not cannot be compared, so
/// `requested` wins and the override is logged rather than silent.
fn monotonic_depth(
    ladder: &[String],
    existing: Option<&str>,
    requested: String,
    asset: &Asset,
) -> String {
    let Some(existing) = existing else {
        return requested;
    };
    let rung = |d: &str| ladder.iter().position(|r| r == d);
    let Some(existing_rung) = rung(existing) else {
        tracing::warn!(
            asset = %asset.id.0,
            kind = %asset.kind,
            existing_depth = existing,
            requested_depth = %requested,
            "asset's existing depth is not on its profile's depth ladder; the requested depth wins"
        );
        return requested;
    };
    // `requested` was validated against this ladder, so it always has a rung.
    match rung(&requested) {
        Some(requested_rung) if requested_rung < existing_rung => existing.to_string(),
        _ => requested,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asset::{read_asset_graph, Coordinate};
    use crate::ledger::events::Surface;
    use crate::report::FindingWriteOptions;
    use std::sync::Arc;

    fn attribution() -> Attribution {
        Attribution {
            run_id: "r".to_string(),
            model: "m".to_string(),
            surface: Surface::Workflow,
            codename: None,
            agent: None,
            provider: None,
        }
    }

    fn opts(ids: &[&str]) -> FindingWriteOptions {
        let ids: Vec<String> = ids.iter().map(|s| s.to_string()).collect();
        let set = crate::profile::builtin_registry()
            .unwrap()
            .active_set(&ids)
            .unwrap();
        FindingWriteOptions::default().with_engagement(Some(Arc::new(set)))
    }

    fn locator() -> Locator {
        Locator(vec![
            Coordinate::Sha256("ab".repeat(32)),
            Coordinate::Address(0x401000),
            Coordinate::Symbol("main".into()),
        ])
    }

    fn input(kind: &str, depth: &str) -> AssetMarkInput {
        AssetMarkInput {
            kind: kind.into(),
            locator: locator(),
            parent: None,
            label: None,
            depth: depth.into(),
        }
    }

    fn paths(ws: &tempfile::TempDir) -> CoveragePaths {
        CoveragePaths::new(ws.path(), "t")
    }

    #[test]
    fn marking_a_function_registers_it_with_that_depth() {
        let ws = tempfile::TempDir::new().unwrap();
        let paths = paths(&ws);
        let out = asset_mark(
            &paths,
            attribution(),
            input("binary:function", "analyzed"),
            &opts(&["binary"]),
        )
        .expect("a rung of the binary ladder on a binary kind is accepted");
        assert_eq!(out.kind, "binary:function");
        assert_eq!(out.depth, "analyzed");

        let graph = read_asset_graph(&paths);
        let assets: Vec<_> = graph.iter().collect();
        assert_eq!(assets.len(), 1);
        assert_eq!(assets[0].id.0, out.id);
        assert_eq!(assets[0].kind, "binary:function");
        assert_eq!(assets[0].depth.as_deref(), Some("analyzed"));
        // The label comes from the kind's template, as report_finding's does.
        assert_eq!(assets[0].label, "main @ 0x401000");
    }

    #[test]
    fn an_unknown_depth_is_rejected_naming_the_ladder_and_writes_nothing() {
        let ws = tempfile::TempDir::new().unwrap();
        let paths = paths(&ws);
        let err = asset_mark(
            &paths,
            attribution(),
            input("binary:function", "bogus"),
            &opts(&["binary"]),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("bogus"), "{err}");
        assert!(err.contains("located, disassembled, analyzed"), "{err}");
        assert!(!paths.assets.exists(), "nothing written on rejection");
    }

    #[test]
    fn a_kind_outside_the_active_set_is_rejected_naming_the_active_profiles() {
        let ws = tempfile::TempDir::new().unwrap();
        let paths = paths(&ws);
        // `code` is active, `binary` is not.
        let err = asset_mark(
            &paths,
            attribution(),
            input("binary:function", "analyzed"),
            &opts(&["code"]),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("binary:function"), "{err}");
        assert!(err.contains("code"), "{err}");
        assert!(!paths.assets.exists());
    }

    #[test]
    fn a_kind_its_owner_does_not_declare_is_rejected() {
        let ws = tempfile::TempDir::new().unwrap();
        let paths = paths(&ws);
        // Routes to `binary` by namespace, but `binary` declares no `nope`.
        let err = asset_mark(
            &paths,
            attribution(),
            input("binary:nope", "analyzed"),
            &opts(&["binary"]),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("binary:nope"), "{err}");
        assert!(
            err.contains("binary:function"),
            "names what it does declare: {err}"
        );
        assert!(!paths.assets.exists());
    }

    #[test]
    fn a_depth_from_another_profiles_ladder_is_rejected() {
        let ws = tempfile::TempDir::new().unwrap();
        let paths = paths(&ws);
        // `reviewed` is `code`'s rung; `binary`'s ladder does not have it.
        let err = asset_mark(
            &paths,
            attribution(),
            input("binary:function", "reviewed"),
            &opts(&["code", "binary"]),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("reviewed"), "{err}");
        assert!(!paths.assets.exists());
    }

    #[test]
    fn an_empty_locator_is_rejected() {
        let ws = tempfile::TempDir::new().unwrap();
        let paths = paths(&ws);
        let mut i = input("binary:function", "analyzed");
        i.locator = Locator(vec![]);
        let err = asset_mark(&paths, attribution(), i, &opts(&["binary"]))
            .unwrap_err()
            .to_string();
        assert!(err.contains("locator"), "{err}");
        assert!(!paths.assets.exists());
    }

    #[test]
    fn marking_without_an_active_engagement_is_rejected() {
        let ws = tempfile::TempDir::new().unwrap();
        let paths = paths(&ws);
        let err = asset_mark(
            &paths,
            attribution(),
            input("binary:function", "analyzed"),
            &FindingWriteOptions::default(),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("engagement"), "{err}");
        assert!(!paths.assets.exists());
    }

    #[test]
    fn re_marking_advances_the_depth_and_keeps_the_other_fields() {
        let ws = tempfile::TempDir::new().unwrap();
        let paths = paths(&ws);
        let opts = opts(&["binary"]);

        // An earlier pass registered the function with a parent, a label and
        // an attribute.
        let parent = Asset::new(
            "binary:binary",
            Locator(vec![Coordinate::Sha256("ab".repeat(32))]),
            "the binary",
        );
        let mut existing = Asset::new("binary:function", locator(), "entry point");
        existing.parent = Some(parent.id.clone());
        existing.depth = Some("located".into());
        existing
            .attributes
            .insert("calling_convention".into(), serde_json::json!("sysv"));
        crate::asset::append_asset(&paths, &existing, &attribution()).unwrap();

        // Re-marking names neither parent nor label: both survive, so does the
        // attribute, and the depth moves up the ladder.
        let out = asset_mark(
            &paths,
            attribution(),
            input("binary:function", "disassembled"),
            &opts,
        )
        .unwrap();
        assert_eq!(
            out.depth, "disassembled",
            "a forward mark is applied as asked"
        );
        let graph = read_asset_graph(&paths);
        assert_eq!(graph.iter().count(), 1);
        let a = graph.get(&existing.id).unwrap();
        assert_eq!(a.depth.as_deref(), Some("disassembled"));
        assert_eq!(a.parent.as_ref(), Some(&parent.id));
        assert_eq!(a.label, "entry point");
        assert_eq!(a.attributes["calling_convention"], "sysv");

        // Advancing again keeps them all.
        let out = asset_mark(
            &paths,
            attribution(),
            input("binary:function", "analyzed"),
            &opts,
        )
        .unwrap();
        assert_eq!(out.depth, "analyzed");
        let graph = read_asset_graph(&paths);
        let a = graph.get(&existing.id).unwrap();
        assert_eq!(a.depth.as_deref(), Some("analyzed"));
        assert_eq!(a.parent.as_ref(), Some(&parent.id));
        assert_eq!(a.attributes["calling_convention"], "sysv");

        // An explicit label and parent DO override, even when the depth part
        // of the same mark is a (clamped) no-op.
        let other_parent = Asset::new(
            "binary:binary",
            Locator(vec![Coordinate::Sha256("cd".repeat(32))]),
            "other",
        );
        let mut i = input("binary:function", "disassembled");
        i.label = Some("renamed".into());
        i.parent = Some(other_parent.id.0.clone());
        let out = asset_mark(&paths, attribution(), i, &opts).unwrap();
        assert_eq!(out.depth, "analyzed", "the clamp does not block the rest");
        let graph = read_asset_graph(&paths);
        let a = graph.get(&existing.id).unwrap();
        assert_eq!(a.label, "renamed");
        assert_eq!(a.parent.as_ref(), Some(&other_parent.id));
        assert_eq!(a.depth.as_deref(), Some("analyzed"));
    }

    #[test]
    fn a_shallower_mark_is_clamped_and_the_output_reports_the_deeper_depth() {
        let ws = tempfile::TempDir::new().unwrap();
        let paths = paths(&ws);
        let opts = opts(&["binary"]);

        let first = asset_mark(
            &paths,
            attribution(),
            input("binary:function", "analyzed"),
            &opts,
        )
        .unwrap();
        assert_eq!(first.depth, "analyzed");

        // `located` is the shallowest rung of binary's ladder: depth never
        // regresses, so the stored rung stays `analyzed` and the tool SAYS so.
        let second = asset_mark(
            &paths,
            attribution(),
            input("binary:function", "located"),
            &opts,
        )
        .unwrap();
        assert_eq!(second.id, first.id);
        assert_eq!(
            second.depth, "analyzed",
            "the output reports the effective (retained) depth, not the request"
        );
        let graph = read_asset_graph(&paths);
        assert_eq!(graph.iter().count(), 1);
        assert_eq!(
            graph.iter().next().unwrap().depth.as_deref(),
            Some("analyzed")
        );
        assert_eq!(
            std::fs::read_to_string(&paths.assets)
                .unwrap()
                .lines()
                .count(),
            1,
            "a fully clamped re-mark changes nothing, so appends nothing"
        );
    }

    #[test]
    fn an_existing_depth_off_the_ladder_yields_to_the_requested_one() {
        let ws = tempfile::TempDir::new().unwrap();
        let paths = paths(&ws);
        // A record whose depth is not a rung of the current ladder (say the
        // profile was edited since): it cannot be ordered against the request.
        let mut existing = Asset::new("binary:function", locator(), "entry point");
        existing.depth = Some("retired-rung".into());
        crate::asset::append_asset(&paths, &existing, &attribution()).unwrap();

        let out = asset_mark(
            &paths,
            attribution(),
            input("binary:function", "located"),
            &opts(&["binary"]),
        )
        .unwrap();
        assert_eq!(out.depth, "located", "the requested depth wins");
        let graph = read_asset_graph(&paths);
        assert_eq!(
            graph.get(&existing.id).unwrap().depth.as_deref(),
            Some("located")
        );
    }

    #[test]
    fn re_marking_the_same_depth_appends_nothing() {
        let ws = tempfile::TempDir::new().unwrap();
        let paths = paths(&ws);
        let opts = opts(&["binary"]);
        for _ in 0..3 {
            asset_mark(
                &paths,
                attribution(),
                input("binary:function", "analyzed"),
                &opts,
            )
            .unwrap();
        }
        assert_eq!(
            std::fs::read_to_string(&paths.assets)
                .unwrap()
                .lines()
                .count(),
            1
        );
    }

    #[test]
    fn input_deserializes_with_optional_parent_and_label() {
        let i: AssetMarkInput = serde_json::from_value(serde_json::json!({
            "kind": "binary:function",
            "locator": [{"address": 4198400}],
            "depth": "located",
        }))
        .unwrap();
        assert!(i.parent.is_none() && i.label.is_none());
        assert_eq!(i.depth, "located");
    }
}

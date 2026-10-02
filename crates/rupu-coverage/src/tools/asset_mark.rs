//! `asset_mark`: record how deeply an engagement asset has been examined, as a
//! rung of its profile's coverage depth ladder. Monotonic — marking a shallower
//! rung after a deeper one keeps the deeper one, and the effective rung is
//! returned so the agent sees what stuck.

use crate::asset::{Asset, Coordinate, Locator};
use crate::ledger::paths::CoveragePaths;
use crate::profile::ActiveSet;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetMarkInput {
    /// Profile-namespaced asset kind, e.g. `"network:service"`.
    pub kind: String,
    #[serde(default)]
    pub coordinates: Vec<Coordinate>,
    /// The depth-ladder rung reached, e.g. `"tested"`.
    pub depth: String,
    #[serde(default)]
    pub label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetMarkOutput {
    pub id: String,
    /// The rung in effect after the monotonic clamp (never shallower than what
    /// was already recorded).
    pub effective_depth: String,
}

#[derive(Debug, thiserror::Error)]
pub enum AssetMarkError {
    #[error("asset kind `{kind}` is not owned by any active engagement profile ({active})")]
    UnownedKind { kind: String, active: String },
    #[error("depth `{depth}` is not a rung of profile `{profile}`'s ladder ({ladder})")]
    UnknownDepth {
        profile: String,
        depth: String,
        ladder: String,
    },
    #[error("asset store: {0}")]
    AssetStore(#[from] crate::asset::AssetStoreError),
}

pub fn asset_mark(
    paths: &CoveragePaths,
    input: AssetMarkInput,
    engagement: &ActiveSet,
) -> Result<AssetMarkOutput, AssetMarkError> {
    let profile =
        engagement
            .profile_for_kind(&input.kind)
            .ok_or_else(|| AssetMarkError::UnownedKind {
                kind: input.kind.clone(),
                active: engagement.ids().join(", "),
            })?;
    let ladder = &profile.coverage.depth_ladder;
    let new_idx = ladder
        .iter()
        .position(|r| r == &input.depth)
        .ok_or_else(|| AssetMarkError::UnknownDepth {
            profile: profile.id.clone(),
            depth: input.depth.clone(),
            ladder: ladder.join(" → "),
        })?;

    let locator = Locator(input.coordinates.clone());
    let id = Asset::derive_id(&input.kind, &locator);

    // Monotonic clamp: if this asset already sits at a deeper rung, keep it.
    let existing_idx = crate::asset::read_assets(&paths.assets)?
        .into_iter()
        .find(|a| a.id == id)
        .and_then(|a| a.depth)
        .and_then(|d| ladder.iter().position(|r| r == &d));
    let effective_idx = existing_idx.map_or(new_idx, |e| e.max(new_idx));
    let effective_depth = ladder[effective_idx].clone();

    let label = input.label.clone().unwrap_or_else(|| input.kind.clone());
    let mut asset = Asset::new(input.kind.clone(), locator, label, None);
    asset.depth = Some(effective_depth.clone());
    crate::ledger::stream::append_record(paths, crate::ledger::stream::Ledger::Assets, &asset)
        .map_err(crate::asset::AssetStoreError::Io)?;

    Ok(AssetMarkOutput {
        id,
        effective_depth,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn active() -> ActiveSet {
        crate::profile::builtin_registry()
            .unwrap()
            .active_set(&["code".into()])
            .unwrap()
    }

    fn paths() -> (tempfile::TempDir, CoveragePaths) {
        let tmp = tempfile::tempdir().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "t");
        (tmp, paths)
    }

    #[test]
    fn unknown_kind_and_unknown_depth_are_errors() {
        let (_t, p) = paths();
        let eng = active();
        assert!(matches!(
            asset_mark(
                &p,
                AssetMarkInput {
                    kind: "network:service".into(),
                    coordinates: vec![],
                    depth: "tested".into(),
                    label: None
                },
                &eng
            ),
            Err(AssetMarkError::UnownedKind { .. })
        ));
        assert!(matches!(
            asset_mark(
                &p,
                AssetMarkInput {
                    kind: "code:file".into(),
                    coordinates: vec![Coordinate::Path("a.rs".into())],
                    depth: "exploited".into(),
                    label: None
                },
                &eng
            ),
            Err(AssetMarkError::UnknownDepth { .. })
        ));
    }

    #[test]
    fn a_mark_is_streamed_as_an_assets_line() {
        use crate::ledger::stream::{RunStream, StreamLine};
        let tmp = tempfile::tempdir().unwrap();
        let stream = tmp.path().join("runs/run_1/coverage.jsonl");
        let p = CoveragePaths::new(tmp.path(), "t").with_run_stream(Some(RunStream {
            path: stream.clone(),
            scope_name: "sec".into(),
        }));
        let out = asset_mark(
            &p,
            AssetMarkInput {
                kind: "code:file".into(),
                coordinates: vec![Coordinate::Path("a.rs".into())],
                depth: "reviewed".into(),
                label: None,
            },
            &active(),
        )
        .unwrap();

        let text = std::fs::read_to_string(&stream).unwrap();
        let lines: Vec<StreamLine> = text
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        match lines.as_slice() {
            [StreamLine::Assets { scope_name, record }] => {
                assert_eq!(scope_name, "sec");
                assert_eq!(record.id, out.id);
                assert_eq!(record.depth.as_deref(), Some("reviewed"));
            }
            other => panic!("expected one assets line, got {other:?}"),
        }
    }

    #[test]
    fn depth_is_monotonic() {
        // use a ladder with >=2 rungs: code is unreviewed -> reviewed
        let (_t, p) = paths();
        let eng = active();
        let mk = |depth: &str| AssetMarkInput {
            kind: "code:file".into(),
            coordinates: vec![Coordinate::Path("a.rs".into())],
            depth: depth.into(),
            label: None,
        };
        let o = asset_mark(&p, mk("reviewed"), &eng).unwrap();
        assert_eq!(o.effective_depth, "reviewed");
        // marking the shallower rung afterwards keeps "reviewed"
        let o2 = asset_mark(&p, mk("unreviewed"), &eng).unwrap();
        assert_eq!(o2.effective_depth, "reviewed", "must not regress");
        assert_eq!(o.id, o2.id);
        // and the stored asset reflects the deeper rung
        let stored = crate::asset::read_assets(&p.assets).unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].depth.as_deref(), Some("reviewed"));
    }
}

use rupu_coverage::{read_assets, ActiveSet, Asset, CoveragePaths};
use serde::Deserialize;
use thiserror::Error;

/// Required engagement-wide coverage threshold: the fraction of discovered
/// assets (of the enumerated kinds) that must have reached a minimum depth.
/// Evaluated by [`CoverageEvaluator`].
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoverageTarget {
    pub reach: f64,
    #[serde(default)]
    pub depth: Option<String>,
    #[serde(default)]
    pub kinds: Option<Vec<String>>,
}

#[derive(Debug, Error)]
pub enum CoverageEvalError {
    #[error("reading assets: {0}")]
    Io(String),
    #[error("{0}")]
    Bad(String),
}

/// The result of evaluating a [`CoverageTarget`] against the asset ledger.
#[derive(Debug, Clone)]
pub struct CoverageOutcome {
    /// `fraction >= target.reach`.
    pub met: bool,
    /// Overall covered / discovered across every evaluated kind.
    pub fraction: f64,
    /// Per-kind fraction, for kinds with at least one discovered asset.
    pub per_kind: Vec<(String, f64)>,
}

/// `(covered, total)` assets of `namespaced_kind` whose depth rung index is
/// `>= index(min_depth)` on `ladder`. An asset with no depth, or a depth that
/// is not on the ladder, is counted in `total` but never `covered`; a
/// `min_depth` that is not on the ladder covers nothing.
fn fraction_at_depth(
    assets: &[Asset],
    namespaced_kind: &str,
    min_depth: &str,
    ladder: &[String],
) -> (u64, u64) {
    let min = ladder.iter().position(|r| r == min_depth);
    let mut covered = 0u64;
    let mut total = 0u64;
    for a in assets.iter().filter(|a| a.kind == namespaced_kind) {
        total += 1;
        let idx = a
            .depth
            .as_ref()
            .and_then(|d| ladder.iter().position(|r| r == d));
        if let (Some(i), Some(m)) = (idx, min) {
            if i >= m {
                covered += 1;
            }
        }
    }
    (covered, total)
}

/// Evaluates engagement-wide coverage: the fraction of discovered assets of
/// the enumerated kinds that have reached a minimum depth.
pub struct CoverageEvaluator;

impl CoverageEvaluator {
    /// For each active profile, take the target's explicit `kinds` (or else the
    /// profile's `coverage.enumerates`) and count assets of each at or above
    /// `target.depth` (or else the ladder terminal). `fraction` is the overall
    /// covered/discovered ratio; `met` is `fraction >= target.reach`.
    ///
    /// `0/0` (nothing of the enumerated kinds discovered yet) is `0.0`, so a
    /// coverage target never fires before there is anything to cover.
    pub fn evaluate(
        target: &CoverageTarget,
        paths: &CoveragePaths,
        active: &ActiveSet,
    ) -> Result<CoverageOutcome, CoverageEvalError> {
        let assets =
            read_assets(&paths.assets).map_err(|e| CoverageEvalError::Io(e.to_string()))?;
        let mut covered_total = 0u64;
        let mut all_total = 0u64;
        let mut per_kind = Vec::new();
        for profile in active.profiles() {
            // `enumerates` holds BARE kind ids; asset kinds are namespaced
            // `"{profile.id}:{kind}"`. Explicit `target.kinds` entries may be
            // bare (namespaced here) or already namespaced (accepted as given,
            // but only evaluated under the profile that owns the namespace so
            // they are neither double-counted nor measured on another
            // profile's depth ladder).
            let kinds: Vec<String> = match &target.kinds {
                Some(explicit) => explicit.clone(),
                None => profile.coverage.enumerates.clone(),
            };
            let depth = target
                .depth
                .clone()
                .or_else(|| profile.coverage.depth_ladder.last().cloned());
            let Some(depth) = depth else { continue };
            for k in kinds {
                let nk = match k.split_once(':') {
                    Some((ns, _)) if ns != profile.id => continue,
                    Some(_) => k.clone(),
                    None => format!("{}:{}", profile.id, k),
                };
                let (c, t) =
                    fraction_at_depth(&assets, &nk, &depth, &profile.coverage.depth_ladder);
                if t > 0 {
                    per_kind.push((nk, c as f64 / t as f64));
                }
                covered_total += c;
                all_total += t;
            }
        }
        let fraction = if all_total == 0 {
            0.0
        } else {
            covered_total as f64 / all_total as f64
        };
        Ok(CoverageOutcome {
            met: fraction >= target.reach,
            fraction,
            per_kind,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rupu_coverage::{Coordinate, Locator, Proto};

    fn host_asset(host: &str, depth: Option<&str>) -> Asset {
        let mut a = Asset::new(
            "network:host",
            Locator(vec![Coordinate::Host(host.into())]),
            host,
            None,
        );
        a.depth = depth.map(|d| d.to_string());
        a
    }

    fn ladder() -> Vec<String> {
        ["discovered", "enumerated", "tested", "exploited"]
            .iter()
            .map(|s| s.to_string())
            .collect()
    }

    #[test]
    fn fraction_counts_enumerated_kind_at_or_above_depth() {
        let assets = vec![
            host_asset("a", Some("tested")),
            host_asset("b", Some("exploited")),
            host_asset("c", Some("discovered")), // below "tested"
        ];
        let (covered, total) = fraction_at_depth(&assets, "network:host", "tested", &ladder());
        assert_eq!((covered, total), (2, 3));
    }

    fn service_asset(host: &str, port: u16, depth: Option<&str>) -> Asset {
        let mut a = Asset::new(
            "network:service",
            Locator(vec![
                Coordinate::Host(host.into()),
                Coordinate::Port {
                    number: port,
                    proto: Proto::Tcp,
                },
            ]),
            format!("{host}:{port}"),
            None,
        );
        a.depth = depth.map(|d| d.to_string());
        a
    }

    fn seed(assets: &[Asset]) -> (tempfile::TempDir, CoveragePaths) {
        let dir = tempfile::tempdir().unwrap();
        let paths = CoveragePaths::new(dir.path(), "t");
        paths.ensure_dir().unwrap();
        let body: String = assets
            .iter()
            .map(|a| serde_json::to_string(a).unwrap() + "\n")
            .collect();
        std::fs::write(&paths.assets, body).unwrap();
        (dir, paths)
    }

    fn active(ids: &[&str]) -> ActiveSet {
        let ids: Vec<String> = ids.iter().map(|s| s.to_string()).collect();
        rupu_coverage::builtin_registry()
            .unwrap()
            .active_set(&ids)
            .unwrap()
    }

    fn target(reach: f64, depth: Option<&str>, kinds: Option<&[&str]>) -> CoverageTarget {
        CoverageTarget {
            reach,
            depth: depth.map(String::from),
            kinds: kinds.map(|ks| ks.iter().map(|k| k.to_string()).collect()),
        }
    }

    #[test]
    fn evaluate_namespaces_bare_enumerated_kinds_and_defaults_to_ladder_terminal() {
        // network enumerates the BARE kinds ["host", "service"]; the assets are
        // namespaced `network:host` / `network:service`. Default depth is the
        // ladder terminal ("exploited").
        let (_dir, paths) = seed(&[
            host_asset("a", Some("tested")),
            host_asset("b", Some("exploited")),
            host_asset("c", Some("discovered")),
            service_asset("b", 22, Some("exploited")),
        ]);
        let out =
            CoverageEvaluator::evaluate(&target(0.5, None, None), &paths, &active(&["network"]))
                .unwrap();
        assert_eq!(out.fraction, 0.5); // 2 of 4 at "exploited"
        assert!(out.met);
        let kinds: Vec<&str> = out.per_kind.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(kinds, ["network:host", "network:service"]);

        let out =
            CoverageEvaluator::evaluate(&target(0.6, None, None), &paths, &active(&["network"]))
                .unwrap();
        assert!(!out.met);
    }

    #[test]
    fn evaluate_empty_ledger_is_zero_not_met() {
        let (_dir, paths) = seed(&[]);
        let out =
            CoverageEvaluator::evaluate(&target(0.5, None, None), &paths, &active(&["network"]))
                .unwrap();
        assert_eq!(out.fraction, 0.0);
        assert!(!out.met);
        assert!(out.per_kind.is_empty());
    }

    #[test]
    fn evaluate_explicit_namespaced_kind_counts_once_across_profiles() {
        let (_dir, paths) = seed(&[
            host_asset("a", Some("tested")),
            host_asset("b", Some("exploited")),
            host_asset("c", Some("discovered")),
        ]);
        let out = CoverageEvaluator::evaluate(
            &target(0.5, Some("tested"), Some(&["network:host"])),
            &paths,
            &active(&["network", "web"]),
        )
        .unwrap();
        // 2 of 3, counted under `network` only (not re-measured on web's ladder).
        assert!((out.fraction - 2.0 / 3.0).abs() < 1e-9);
        assert!(out.met);
        assert_eq!(out.per_kind.len(), 1);
    }
}

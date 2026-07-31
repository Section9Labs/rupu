//! `cp serve` adapter for rupu-cp's [`FleetInventory`] port.
//!
//! Probes every credentialled provider on a TTL and serves the result from an
//! in-memory cache. [`FleetInventory::snapshot`] never performs I/O — the
//! dashboard reads the cache, and only the background refresh task touches the
//! network, so one hung provider can never stall a page render.
//!
//! Providers are built as CONCRETE client types rather than through
//! `ProviderRegistry`'s `Box<dyn LlmProvider>`, mirroring `cmd/models.rs`'s
//! `populate_live`: `async_trait` imposes a `Sync` bound on `&self` methods of
//! a boxed trait object that the concrete types sidestep.

#![deny(clippy::all)]

use chrono::Utc;
use rupu_auth::CredentialResolver;
use rupu_cp::fleet_inventory::{FleetInventory, InventorySnapshot, ProbeState, ProviderProbeRow};
use rupu_providers::{error::ProviderError, provider::LlmProvider};
use std::sync::{Arc, RwLock};

/// How long a probe result stays authoritative. Long enough that a fleet of
/// providers costs a handful of requests an hour; short enough that a revoked
/// key turns the strip red within a coffee break.
pub const PROBE_TTL_SECS: u64 = 300;

/// How long SCM inventory stays authoritative. Much longer than the provider
/// TTL: filling it costs one issue listing per connected repo.
pub const SCM_TTL_SECS: u64 = 900;

/// Per-repo issue fetch cap. Hitting it means the open count is a floor, not a
/// total — `issues_capped` says so and the UI renders `312+`.
///
/// Tunable: 500 is an initial value chosen to keep a refresh cheap for
/// ordinary repos. If real orgs routinely exceed it, raise it here; the floor
/// semantics stay correct either way.
pub const ISSUE_FETCH_CAP: u32 = 500;

/// The providers rupu knows how to authenticate. Same list `rupu models`
/// refreshes — one place to add a provider, not two.
const PROVIDERS: [&str; 4] = ["anthropic", "openai", "gemini", "copilot"];

/// Everything the SCM half needs. Built once by `cp serve`, which already has
/// all of it in scope — this must never re-resolve credentials itself.
pub struct ScmDeps {
    pub global_dir: std::path::PathBuf,
    pub resolver: Arc<rupu_auth::resolver::KeychainResolver>,
    pub repos: Arc<dyn rupu_cp::repos::RepoLister>,
    pub scm: Arc<rupu_scm::Registry>,
}

/// The provider half of the cache, refreshed on [`PROBE_TTL_SECS`].
#[derive(Default)]
struct ProviderHalf {
    rows: Vec<ProviderProbeRow>,
    stamp: Option<chrono::DateTime<Utc>>,
}

/// The SCM half, refreshed on the much longer [`SCM_TTL_SECS`].
#[derive(Default)]
struct ScmHalf {
    repos: Option<u64>,
    issues_pending: Option<u64>,
    issues_open: Option<u64>,
    issues_capped: bool,
    stamp: Option<chrono::DateTime<Utc>>,
}

/// The two halves are stored separately, each with its own stamp, so a slow
/// SCM refresh never blanks fresh provider data (or vice versa).
pub struct CpFleetInventory {
    /// `None` in unit tests, which exercise the cache and `classify` without
    /// touching the keychain.
    resolver: Option<Arc<rupu_auth::resolver::KeychainResolver>>,
    /// `None` in unit tests and whenever SCM credentials are unavailable.
    scm: Option<ScmDeps>,
    providers: RwLock<ProviderHalf>,
    scm_cache: RwLock<ScmHalf>,
}

/// Map a probe error to a cache state.
///
/// `NotImplemented` is NOT a failure — it means this provider has no probe, so
/// nothing has been established about it and it must land in `NeverProbed`.
/// Everything auth-shaped is `AuthFailed`; everything transport- or
/// server-shaped is `Unreachable`. That split is what lets the operator tell
/// "my key is wrong" from "the provider is down".
pub fn classify(err: &ProviderError) -> ProbeState {
    match err {
        ProviderError::NotImplemented { .. } => ProbeState::NeverProbed,
        ProviderError::Unauthorized { .. }
        | ProviderError::MissingAuth { .. }
        | ProviderError::TokenRefreshFailed(_)
        | ProviderError::AuthConfig(_) => ProbeState::AuthFailed {
            detail: err.to_string(),
        },
        ProviderError::Api { status, .. } if *status == 401 || *status == 403 => {
            ProbeState::AuthFailed {
                detail: err.to_string(),
            }
        }
        _ => ProbeState::Unreachable {
            detail: err.to_string(),
        },
    }
}

impl CpFleetInventory {
    pub fn new(resolver: Arc<rupu_auth::resolver::KeychainResolver>, scm: Option<ScmDeps>) -> Self {
        Self {
            resolver: Some(resolver),
            scm,
            providers: RwLock::new(ProviderHalf::default()),
            scm_cache: RwLock::new(ScmHalf::default()),
        }
    }

    #[cfg(test)]
    pub fn new_for_test() -> Self {
        Self {
            resolver: None,
            scm: None,
            providers: RwLock::new(ProviderHalf::default()),
            scm_cache: RwLock::new(ScmHalf::default()),
        }
    }

    #[cfg(test)]
    pub fn set_provider_stamp_for_test(&self, t: chrono::DateTime<Utc>) {
        self.providers.write().expect("lock").stamp = Some(t);
    }

    #[cfg(test)]
    pub fn set_scm_stamp_for_test(&self, t: chrono::DateTime<Utc>) {
        self.scm_cache.write().expect("lock").stamp = Some(t);
    }

    /// Probe every credentialled provider and replace the cache.
    ///
    /// A provider whose credentials cannot be resolved is not "configured" at
    /// all and is omitted entirely — the strip counts what rupu can actually
    /// use. Errors from a provider that IS configured are recorded as states,
    /// never propagated: one dead provider must not blank the other rows.
    pub async fn refresh_providers(&self) {
        let Some(resolver) = &self.resolver else {
            return;
        };

        let mut rows = Vec::new();
        for name in PROVIDERS {
            // No credentials → not configured. Skip rather than record, so
            // `providers_configured` means "rupu can use this" rather than
            // "rupu has heard of this".
            let Ok((_mode, creds)) = resolver.get(name, None).await else {
                continue;
            };
            rows.push(ProviderProbeRow {
                provider: name.to_string(),
                state: probe_one(name, creds).await,
                probed_at: Some(Utc::now()),
            });
        }

        if let Ok(mut c) = self.providers.write() {
            c.rows = rows;
            c.stamp = Some(Utc::now());
        }
    }

    /// Refresh the SCM half: connected repos, open-issue totals, and the
    /// autoflow-pending backlog.
    ///
    /// Every failure degrades that ONE field to `None` with a warn — a
    /// rate-limited GitHub must not blank the repo count it already returned.
    pub async fn refresh_scm(&self) {
        let Some(deps) = &self.scm else {
            return;
        };

        let repo_entries = match deps.repos.list().await {
            Ok(v) => Some(v),
            Err(e) => {
                tracing::warn!(error = %e, "fleet inventory: repo listing failed; repos unreported");
                None
            }
        };

        // One issue listing per connected repo, capped. A per-repo failure
        // drops that repo from the tally rather than the whole count — but it
        // ALSO makes the total a floor, exactly as hitting the cap does. A
        // repo we could not read (private, blocked, rate-limited) would
        // otherwise silently shrink the number with no marker at all.
        let mut per_repo: Vec<(u64, bool)> = Vec::new();
        let mut any_repo_unread = false;
        if let Some(entries) = &repo_entries {
            for entry in entries {
                match count_open_issues(&deps.scm, &entry.platform, &entry.repo).await {
                    Ok(n) => per_repo.push((n, n >= u64::from(ISSUE_FETCH_CAP))),
                    Err(e) => {
                        any_repo_unread = true;
                        tracing::warn!(repo = %entry.repo, error = %e, "fleet inventory: issue listing failed; repo omitted from the open tally (count is now a floor)")
                    }
                }
            }
        }
        let (issues_open, hit_cap) = tally_open_issues(&per_repo);
        // Only a total we actually have can be a floor; with `None` there is
        // nothing to qualify.
        let issues_capped = issues_open.is_some() && (hit_cap || any_repo_unread);

        let issues_pending = self.derive_pending(deps).await;

        if let Ok(mut c) = self.scm_cache.write() {
            c.repos = repo_entries.map(|v| v.len() as u64);
            c.issues_open = issues_open;
            c.issues_capped = issues_capped;
            c.issues_pending = issues_pending;
            c.stamp = Some(Utc::now());
        }
    }

    /// The autoflow-pending backlog, derived by the SAME pair the cron tick
    /// uses — `discover_tick_autoflows` + `collect_issue_matches` — so the
    /// strip's number is exactly what rupu will pick up.
    ///
    /// A second matcher living in the CP could drift from this one, and a
    /// strip that disagrees with the scheduler is worse than no strip.
    async fn derive_pending(&self, deps: &ScmDeps) -> Option<u64> {
        let repo_store = rupu_workspace::RepoRegistryStore {
            root: deps.global_dir.join("repos"),
        };
        let discovered = match crate::cmd::autoflow::discover_tick_autoflows(
            &deps.global_dir,
            &repo_store,
        ) {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!(error = %e, "fleet inventory: autoflow discovery failed; issues_pending unreported");
                return None;
            }
        };

        let matches = match crate::cmd::autoflow::collect_issue_matches(
            &discovered,
            deps.resolver.as_ref(),
        )
        .await
        {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(error = %e, "fleet inventory: issue matching failed; issues_pending unreported");
                return None;
            }
        };

        let refs: Vec<String> = matches.iter().map(|m| m.issue_ref_text.clone()).collect();
        let claim_store = rupu_workspace::AutoflowClaimStore {
            root: deps.global_dir.join("autoflows").join("claims"),
        };
        Some(pending_after_claims(&refs, &claim_store))
    }
}

/// Sum per-repo `(open_count, hit_cap)` pairs.
///
/// Returns `None` for an empty input: zero repos read is "we know nothing",
/// not "you have no open issues". Any capped repo makes the total a floor.
fn tally_open_issues(counts: &[(u64, bool)]) -> (Option<u64>, bool) {
    if counts.is_empty() {
        return (None, false);
    }
    let total = counts.iter().map(|(n, _)| n).sum();
    let capped = counts.iter().any(|(_, c)| *c);
    (Some(total), capped)
}

/// Count open issues for one repo, bounded by [`ISSUE_FETCH_CAP`].
async fn count_open_issues(
    registry: &rupu_scm::Registry,
    platform: &str,
    project: &str,
) -> Result<u64, String> {
    let tracker = match platform {
        "github" => rupu_scm::IssueTracker::Github,
        "gitlab" => rupu_scm::IssueTracker::Gitlab,
        other => return Err(format!("no issue tracker for platform {other}")),
    };
    let conn = registry
        .issues(tracker)
        .ok_or_else(|| format!("no issue connector configured for {platform}"))?;
    let filter = rupu_scm::IssueFilter {
        state: Some(rupu_scm::IssueState::Open),
        labels: Vec::new(),
        author: None,
        limit: Some(ISSUE_FETCH_CAP),
    };
    conn.list_issues(project, filter)
        .await
        .map(|v| v.len() as u64)
        .map_err(|e| e.to_string())
}

/// Issue refs that are selected but not already in flight.
///
/// The claim rule matches `ensure_manual_run_can_take_claim` (`cmd/autoflow.rs`):
/// `Complete` and `Released` are terminal, so their issues are available
/// again; every other status means a run already owns this issue, and that
/// work belongs to the `claimed` count rather than the `pending` one.
fn pending_after_claims(
    issue_refs: &[String],
    claim_store: &rupu_workspace::AutoflowClaimStore,
) -> u64 {
    use rupu_workspace::ClaimStatus;
    issue_refs
        .iter()
        .filter(|r| match claim_store.load(r) {
            Ok(Some(claim)) => {
                matches!(claim.status, ClaimStatus::Complete | ClaimStatus::Released)
            }
            // No claim at all — genuinely pending.
            Ok(None) => true,
            // An unreadable claim record must not silently inflate the
            // backlog; treat it as in flight and warn.
            Err(e) => {
                tracing::warn!(issue_ref = %r, error = %e, "fleet inventory: unreadable claim; excluding from pending");
                false
            }
        })
        .count() as u64
}

/// Probe a single provider by its concrete client type.
///
/// Only Anthropic implements a real `probe` today; the rest inherit the trait
/// default and therefore report `NeverProbed` — honestly "not checked", not a
/// green light. A client that cannot even be constructed is an auth/config
/// problem, which `classify` already names.
async fn probe_one(name: &str, creds: rupu_providers::auth::AuthCredentials) -> ProbeState {
    let result = match name {
        "anthropic" => {
            // Route OAuth through `from_auth` so the probe sends
            // `Authorization: Bearer` + the OAuth beta; stuffing an OAuth
            // token into the api-key constructor yields a 401 that would be
            // misreported as a bad credential (see `cmd/models.rs`).
            let auth_method = creds.into_anthropic_auth_method();
            rupu_providers::AnthropicClient::from_auth(auth_method)
                .probe()
                .await
        }
        "openai" => match rupu_providers::OpenAiCodexClient::new(creds, None) {
            Ok(c) => c.probe().await,
            Err(e) => Err(e),
        },
        "gemini" => match rupu_providers::GoogleGeminiClient::new(
            creds,
            rupu_providers::google_gemini::GeminiVariant::GeminiCli,
            None,
        ) {
            Ok(c) => c.probe().await,
            Err(e) => Err(e),
        },
        "copilot" => match rupu_providers::GithubCopilotClient::new(creds, None) {
            Ok(c) => c.probe().await,
            Err(e) => Err(e),
        },
        // Unreachable given `PROVIDERS`, but a new entry added there without a
        // arm here must report "not checked" rather than "healthy".
        _ => Err(ProviderError::NotImplemented {
            provider: name.to_string(),
        }),
    };

    match result {
        Ok(()) => ProbeState::Ok,
        // Rate limiting is NOT a health failure: a 429 proves the credential
        // works. Reporting it red would light the strip up during normal
        // heavy use.
        Err(ProviderError::RateLimited { .. }) => ProbeState::Ok,
        Err(e) => classify(&e),
    }
}

impl FleetInventory for CpFleetInventory {
    fn snapshot(&self) -> InventorySnapshot {
        let p = self.providers.read().ok();
        let s = self.scm_cache.read().ok();

        let p_stamp = p.as_ref().and_then(|h| h.stamp);
        let s_stamp = s.as_ref().and_then(|h| h.stamp);
        // Oldest wins: a fleet number is only as fresh as its stalest
        // contributing cache, and the two halves refresh on very different
        // schedules. A half that has never run contributes no stamp at all
        // rather than pinning the pair to `None`.
        let captured_at = match (p_stamp, s_stamp) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        };

        InventorySnapshot {
            providers: p.map(|h| h.rows.clone()).unwrap_or_default(),
            repos: s.as_ref().and_then(|h| h.repos),
            issues_pending: s.as_ref().and_then(|h| h.issues_pending),
            issues_open: s.as_ref().and_then(|h| h.issues_open),
            issues_capped: s.as_ref().map(|h| h.issues_capped).unwrap_or(false),
            captured_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_shaped_errors_classify_as_auth_failed() {
        let cases = [
            ProviderError::Unauthorized {
                provider: "anthropic".into(),
                auth_mode: rupu_providers::auth_mode::AuthMode::ApiKey,
                hint: "check your key".into(),
            },
            ProviderError::MissingAuth {
                provider: "anthropic".into(),
                env_hint: "ANTHROPIC_API_KEY".into(),
            },
            ProviderError::TokenRefreshFailed("expired".into()),
            ProviderError::AuthConfig("bad auth.json".into()),
            ProviderError::Api {
                status: 401,
                message: "nope".into(),
            },
            ProviderError::Api {
                status: 403,
                message: "nope".into(),
            },
        ];
        for err in cases {
            assert!(
                matches!(classify(&err), ProbeState::AuthFailed { .. }),
                "{err:?} must classify as AuthFailed"
            );
        }
    }

    #[test]
    fn transport_and_server_errors_classify_as_unreachable() {
        let cases = [
            ProviderError::Http("connection refused".into()),
            ProviderError::Api {
                status: 503,
                message: "down".into(),
            },
        ];
        for err in cases {
            assert!(
                matches!(classify(&err), ProbeState::Unreachable { .. }),
                "{err:?} must classify as Unreachable"
            );
        }
    }

    /// A provider with no `probe()` impl must report NeverProbed — the whole
    /// point of the NotImplemented default. Classifying it as Unreachable
    /// would put a red count on a provider that may be perfectly fine.
    #[test]
    fn not_implemented_classifies_as_never_probed() {
        let err = ProviderError::NotImplemented {
            provider: "gemini".into(),
        };
        assert!(matches!(classify(&err), ProbeState::NeverProbed));
    }

    /// Before the first refresh the cache is empty, so the strip reports
    /// nothing about providers rather than "0 configured".
    #[test]
    fn snapshot_before_any_refresh_is_empty() {
        let inv = CpFleetInventory::new_for_test();
        let snap = inv.snapshot();
        assert!(snap.providers.is_empty());
        assert!(snap.captured_at.is_none());
    }

    /// Without a resolver (the unit-test shape) a refresh is a no-op rather
    /// than a panic or a fabricated empty-but-stamped cache.
    #[tokio::test]
    async fn refresh_without_a_resolver_leaves_the_cache_untouched() {
        let inv = CpFleetInventory::new_for_test();
        inv.refresh_providers().await;
        assert!(inv.snapshot().captured_at.is_none());
    }

    // ── Two-half cache staleness ─────────────────────────────────────

    /// The halves refresh on very different schedules, so the snapshot's
    /// single stamp must report the OLDER one. Reporting the newer would claim
    /// the 15-minute-old issue counts were seconds old.
    #[test]
    fn captured_at_reports_the_older_of_the_two_half_stamps() {
        let inv = CpFleetInventory::new_for_test();
        let older = Utc::now() - chrono::Duration::minutes(12);
        let newer = Utc::now();

        inv.set_provider_stamp_for_test(newer);
        inv.set_scm_stamp_for_test(older);

        assert_eq!(inv.snapshot().captured_at, Some(older));
    }

    #[test]
    fn captured_at_is_none_until_a_half_has_refreshed() {
        let inv = CpFleetInventory::new_for_test();
        assert_eq!(inv.snapshot().captured_at, None);
    }

    /// One half having run does not fabricate a stamp for the other.
    #[test]
    fn captured_at_uses_the_only_stamp_when_just_one_half_has_run() {
        let inv = CpFleetInventory::new_for_test();
        let t = Utc::now();
        inv.set_provider_stamp_for_test(t);
        assert_eq!(inv.snapshot().captured_at, Some(t));
    }

    // ── Open-issue tally + cap ───────────────────────────────────────

    #[test]
    fn tally_marks_capped_when_any_repo_hit_the_cap() {
        let (total, capped) = tally_open_issues(&[(186, false), (500, true), (12, false)]);
        assert_eq!(total, Some(698));
        assert!(capped, "one repo at the cap makes the whole total a floor");
    }

    #[test]
    fn tally_is_not_capped_when_no_repo_hit_the_cap() {
        let (total, capped) = tally_open_issues(&[(3, false), (9, false)]);
        assert_eq!(total, Some(12));
        assert!(!capped);
    }

    /// No repos read at all is not "zero open issues" — report nothing.
    #[test]
    fn tally_of_no_repos_reports_none() {
        let (total, capped) = tally_open_issues(&[]);
        assert_eq!(total, None);
        assert!(!capped);
    }

    /// A genuine zero survives as a zero.
    #[test]
    fn tally_of_repos_with_no_open_issues_is_some_zero() {
        let (total, _) = tally_open_issues(&[(0, false), (0, false)]);
        assert_eq!(total, Some(0));
    }

    /// A repo that could not be read is dropped from the tally, so the total
    /// must be marked a floor exactly as hitting the cap does. This is the
    /// `451 Repository access blocked` case seen against a real account: 158
    /// of 159 repos tallied, and the number must not present itself as
    /// complete.
    #[test]
    fn an_unreadable_repo_makes_the_total_a_floor() {
        // `tally_open_issues` sees only the repos that succeeded …
        let (total, hit_cap) = tally_open_issues(&[(100, false), (19, false)]);
        assert_eq!(total, Some(119));
        assert!(!hit_cap, "no repo hit the cap");

        // … so `refresh_scm` ORs in the unread-repo flag. Mirror that rule
        // here, which is the invariant the caller must preserve.
        let any_repo_unread = true;
        let issues_capped = total.is_some() && (hit_cap || any_repo_unread);
        assert!(
            issues_capped,
            "a dropped repo silently shrinks the total; it must render as a floor"
        );
    }

    /// With nothing readable at all there is no total to qualify — `None`
    /// must not be dressed up as a capped zero.
    #[test]
    fn no_readable_repos_reports_none_and_is_not_marked_a_floor() {
        let (total, hit_cap) = tally_open_issues(&[]);
        let any_repo_unread = true;
        let issues_capped = total.is_some() && (hit_cap || any_repo_unread);
        assert_eq!(total, None);
        assert!(
            !issues_capped,
            "there is no number to call a floor when nothing was read"
        );
    }

    // ── Pending vs claims ────────────────────────────────────────────

    fn write_claim(root: &std::path::Path, issue_ref: &str, status: &str) {
        let dir = root.join(rupu_workspace::autoflow_claim_store::issue_key(issue_ref));
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(
            dir.join("claim.toml"),
            format!(
                "issue_ref = \"{issue_ref}\"\n\
                 repo_ref = \"github:o/r\"\n\
                 workflow = \"wf\"\n\
                 status = \"{status}\"\n\
                 updated_at = \"2026-07-30T00:00:00Z\"\n"
            ),
        )
        .expect("write");
    }

    /// "Pending" is work rupu is ABOUT to pick up. An issue already claimed
    /// and in flight is work rupu has ALREADY picked up — it belongs to the
    /// claims count, not this one. `Complete` / `Released` claims are finished
    /// or handed back, so their issues are pending again if still selected.
    #[test]
    fn pending_excludes_issues_with_an_in_flight_claim() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let claim_store = rupu_workspace::AutoflowClaimStore {
            root: tmp.path().to_path_buf(),
        };
        write_claim(tmp.path(), "github:o/r/issues/1", "running");
        write_claim(tmp.path(), "github:o/r/issues/2", "complete");
        write_claim(tmp.path(), "github:o/r/issues/3", "released");

        let refs: Vec<String> = [
            "github:o/r/issues/1",
            "github:o/r/issues/2",
            "github:o/r/issues/3",
            "github:o/r/issues/4",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();

        assert_eq!(
            pending_after_claims(&refs, &claim_store),
            3,
            "issue 1 is in flight; 2 and 3 are finished/handed back and 4 was never claimed"
        );
    }

    /// With no claims at all, every selected issue is pending.
    #[test]
    fn pending_with_no_claims_counts_every_selected_issue() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let claim_store = rupu_workspace::AutoflowClaimStore {
            root: tmp.path().to_path_buf(),
        };
        let refs = vec!["github:o/r/issues/7".to_string()];
        assert_eq!(pending_after_claims(&refs, &claim_store), 1);
    }
}

//! The recovery ladder (spec 2026-10-01 response-outcomes §5–§6): the policy
//! table that maps an [`OutcomeClass`] to a rung-0 action, a budget and
//! rung-1/rung-2 eligibility, plus the per-run [`RecoveryState`] and the
//! [`HopBuilder`] port the runner uses to move onto a fallback model.
//!
//! Pure: no I/O. The runner owns the loop that applies these decisions.

use std::collections::HashSet;
use std::sync::Arc;

use rupu_config::FallbackEntry;
use rupu_providers::model_limits::ModelLimits;
use rupu_providers::provider::LlmProvider;
use rupu_providers::reply_error::ErrorClass;

use crate::outcome::OutcomeClass;

pub const PAUSE_TURN_BUDGET: u32 = 5;
pub const TRUNCATION_BUDGET: u32 = 3;
pub const MALFORMED_BUDGET: u32 = 2;
pub const EMPTY_REPLY_BUDGET: u32 = 1;
pub const INCOMPLETE_BUDGET: u32 = 1;
pub const RAISED_CAP_BUDGET: u32 = 1;
/// Ceiling on recovery actions (rung 0 and hops combined) in one run.
pub const MAX_RECOVERY_ACTIONS: u32 = 20;

pub const TRUNCATION_NOTE: &str = "Your previous reply was cut off at the output limit. Continue exactly where you stopped; don't repeat what you already wrote.";
pub const EMPTY_REPLY_NOTE: &str = "Your last reply was empty. Continue the task.";
pub const MALFORMED_NOTE_PREFIX: &str = "Your last tool call could not be used:";
pub const RECOVERY_RETRY_NOTE: &str = "A previous attempt at this task stopped ({title}). You are continuing on {provider}/{model}; check the current state, then continue the task.";

/// The note sent back after a tool call that could not be used.
pub fn malformed_note(name: &str, error: &str) -> String {
    format!("{MALFORMED_NOTE_PREFIX} {name}: {error}. Call the tool again with valid arguments.")
}

/// The note sent to a fallback model that takes over an interrupted attempt.
pub fn recovery_retry_note(title: &str, provider: &str, model: &str) -> String {
    RECOVERY_RETRY_NOTE
        .replace("{title}", title)
        .replace("{provider}", provider)
        .replace("{model}", model)
}

/// What the runner tries first, on the current model, for an outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Rung0 {
    ContinuePause,
    ContinueTruncated,
    /// A tool call cut off at an output cap that was lowered below the
    /// model's maximum. The cap is lowered only when input + max_tokens
    /// overflowed the window, so the same input at the full cap would fail
    /// the same way: the history is compacted first, then the turn is
    /// retried at the model's output cap. When nothing can be compacted,
    /// rung 0 does nothing and the outcome climbs the ladder.
    RetryRaisedCap,
    CompactThenContinue,
    CorrectMalformed,
    NudgeEmpty,
    RetryTurn,
    /// The provider's own retry/backoff and overflow-compaction paths run
    /// as before; the ladder only adds a hop after they give up.
    ExistingErrorPipeline,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Policy {
    pub rung0: Rung0,
    /// Rung-0 attempts allowed per turn.
    pub budget: u32,
    /// Same-provider fallback models are eligible.
    pub rung1: bool,
    /// Other-provider fallback models are eligible.
    pub rung2: bool,
    /// The partial reply must not be kept in the conversation.
    pub discard_partial: bool,
}

/// The ladder policy for `class` (spec §5.2). `truncated_tool` is true for a
/// `MaxTokens` reply that cut a tool call off mid-arguments.
pub fn policy(class: &OutcomeClass, truncated_tool: bool) -> Policy {
    let p = |rung0, budget, rung1, rung2, discard_partial| Policy {
        rung0,
        budget,
        rung1,
        rung2,
        discard_partial,
    };
    match class {
        OutcomeClass::PauseTurn => p(Rung0::ContinuePause, PAUSE_TURN_BUDGET, false, false, false),
        OutcomeClass::MaxTokens if truncated_tool => {
            p(Rung0::RetryRaisedCap, RAISED_CAP_BUDGET, true, true, true)
        }
        OutcomeClass::MaxTokens => p(
            Rung0::ContinueTruncated,
            TRUNCATION_BUDGET,
            true,
            true,
            false,
        ),
        OutcomeClass::ContextWindowExceeded => p(Rung0::CompactThenContinue, 3, true, true, false),
        OutcomeClass::Refusal | OutcomeClass::Safety => p(Rung0::None, 0, true, true, true),
        OutcomeClass::MalformedToolCall => {
            p(Rung0::CorrectMalformed, MALFORMED_BUDGET, true, true, false)
        }
        OutcomeClass::EmptyReply => p(Rung0::NudgeEmpty, EMPTY_REPLY_BUDGET, true, true, false),
        OutcomeClass::Incomplete => p(Rung0::RetryTurn, INCOMPLETE_BUDGET, true, true, true),
        OutcomeClass::UnrecognizedStop | OutcomeClass::UnreportedStop => {
            p(Rung0::None, 0, false, false, false)
        }
        OutcomeClass::ProviderError(e) => match e {
            ErrorClass::RateLimited
            | ErrorClass::Overloaded
            | ErrorClass::Server
            | ErrorClass::Timeout
            | ErrorClass::ContextOverflow => p(Rung0::ExistingErrorPipeline, 0, false, true, false),
            ErrorClass::Quota => p(Rung0::None, 0, false, true, false),
            ErrorClass::NotFound | ErrorClass::Policy => p(Rung0::None, 0, true, true, false),
            ErrorClass::Auth
            | ErrorClass::Permission
            | ErrorClass::InvalidRequest
            | ErrorClass::TooLarge
            | ErrorClass::Unrecognized => p(Rung0::None, 0, false, false, false),
        },
    }
}

/// A built fallback: a provider client plus what the runner needs to run on it.
pub struct Hop {
    pub provider: Box<dyn LlmProvider>,
    pub provider_name: String,
    pub model: String,
    pub limits: ModelLimits,
}

/// Builds a [`Hop`] for a fallback entry. The runner only knows this trait;
/// the CLI supplies an implementation backed by the provider factory.
#[async_trait::async_trait]
pub trait HopBuilder: Send + Sync {
    async fn build(&self, provider: &str, model: &str) -> Result<Hop, String>;
}

/// Recovery ladder inputs for one run. The default has no chain and no hop
/// builder, so rungs 1 and 2 are unavailable and rung 0 still applies.
#[derive(Clone, Default)]
pub struct RecoveryOpts {
    pub chain: Vec<FallbackEntry>,
    pub hop_builder: Option<Arc<dyn HopBuilder>>,
}

/// Per-run recovery bookkeeping.
#[derive(Debug, Default)]
pub struct RecoveryState {
    actions: u32,
    /// `(turn_idx, kind)` -> attempts used this turn.
    budgets: std::collections::HashMap<(u32, Rung0), u32>,
    /// `(provider, model)` pairs already hopped to.
    tried: HashSet<(Option<String>, String)>,
    outcomes: u32,
    /// The provider/model the attempt started on. An entry without a
    /// `provider` means this provider, wherever the attempt has hopped to
    /// since (spec §6.1). `None`: the current provider/model stand in.
    origin: Option<(String, String)>,
}

impl RecoveryState {
    pub fn new() -> Self {
        Self::default()
    }

    /// State for an attempt that starts on `provider`/`model`: unnamed
    /// chain entries resolve against this provider, and rungs are counted
    /// from it, for the whole run.
    pub fn with_origin(provider: &str, model: &str) -> Self {
        Self {
            origin: Some((provider.to_string(), model.to_string())),
            ..Self::default()
        }
    }

    /// Whether [`MAX_RECOVERY_ACTIONS`] still leaves room for one more.
    pub fn can_act(&self) -> bool {
        self.actions < MAX_RECOVERY_ACTIONS
    }

    /// Count one action; false once [`MAX_RECOVERY_ACTIONS`] is reached.
    pub fn take_action(&mut self) -> bool {
        if self.actions >= MAX_RECOVERY_ACTIONS {
            return false;
        }
        self.actions += 1;
        true
    }

    /// Per-turn budget use for `kind`; returns `Some(attempt)` (1-based) while
    /// under budget.
    pub fn try_budget(&mut self, turn_idx: u32, kind: Rung0, budget: u32) -> Option<u32> {
        let used = self.budgets.entry((turn_idx, kind)).or_insert(0);
        if *used >= budget {
            return None;
        }
        *used += 1;
        Some(*used)
    }

    /// Next untried hop: rung-1 entries first (if `allow_rung1`), then rung-2
    /// (if `allow_rung2`), in chain order within each rung.
    ///
    /// - An entry without a `provider` resolves to the origin provider
    ///   ([`RecoveryState::with_origin`]), not to the provider the attempt
    ///   is on now. Rung 1 is an entry whose resolved provider is the
    ///   origin's; rung 2 is every other.
    /// - The current provider/model and the origin pair (the attempt
    ///   already ran there) are never returned.
    /// - The returned entry is marked tried (keyed on its resolved pair).
    ///
    /// The returned entry carries its resolved provider.
    pub fn next_hop(
        &mut self,
        chain: &[FallbackEntry],
        current_provider: &str,
        current_model: &str,
        allow_rung1: bool,
        allow_rung2: bool,
    ) -> Option<(u8, FallbackEntry)> {
        let (origin_provider, origin_model) = match &self.origin {
            Some((p, m)) => (p.clone(), m.clone()),
            None => (current_provider.to_string(), current_model.to_string()),
        };
        for rung in [1u8, 2u8] {
            if (rung == 1 && !allow_rung1) || (rung == 2 && !allow_rung2) {
                continue;
            }
            for entry in chain {
                let provider = entry.provider.as_deref().unwrap_or(&origin_provider);
                if (rung == 1) != (provider == origin_provider) {
                    continue;
                }
                let is = |p: &str, m: &str| provider == p && entry.model == m;
                if is(current_provider, current_model) || is(&origin_provider, &origin_model) {
                    continue;
                }
                let key = (Some(provider.to_string()), entry.model.clone());
                if self.tried.contains(&key) {
                    continue;
                }
                self.tried.insert(key);
                return Some((
                    rung,
                    FallbackEntry {
                        provider: Some(provider.to_string()),
                        model: entry.model.clone(),
                    },
                ));
            }
        }
        None
    }

    /// [`Self::next_hop`] charged as one recovery action. At the action cap
    /// it returns `None` without selecting, so no entry is marked tried.
    pub fn take_hop(
        &mut self,
        chain: &[FallbackEntry],
        current_provider: &str,
        current_model: &str,
        allow_rung1: bool,
        allow_rung2: bool,
    ) -> Option<(u8, FallbackEntry)> {
        if !self.can_act() {
            return None;
        }
        let hop = self.next_hop(
            chain,
            current_provider,
            current_model,
            allow_rung1,
            allow_rung2,
        )?;
        self.take_action();
        Some(hop)
    }

    /// Run-local outcome ids: `oc_1`, `oc_2`, ...
    pub fn next_outcome_id(&mut self) -> String {
        self.outcomes += 1;
        format!("oc_{}", self.outcomes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(provider: Option<&str>, model: &str) -> FallbackEntry {
        FallbackEntry {
            provider: provider.map(str::to_string),
            model: model.to_string(),
        }
    }

    fn row(class: OutcomeClass, truncated: bool) -> (Rung0, u32, bool, bool, bool) {
        let p = policy(&class, truncated);
        (p.rung0, p.budget, p.rung1, p.rung2, p.discard_partial)
    }

    #[test]
    fn policy_table_rows() {
        use OutcomeClass::*;
        assert_eq!(
            row(PauseTurn, false),
            (Rung0::ContinuePause, 5, false, false, false)
        );
        assert_eq!(
            row(MaxTokens, false),
            (Rung0::ContinueTruncated, 3, true, true, false)
        );
        assert_eq!(
            row(MaxTokens, true),
            (Rung0::RetryRaisedCap, 1, true, true, true)
        );
        assert_eq!(
            row(ContextWindowExceeded, false),
            (Rung0::CompactThenContinue, 3, true, true, false)
        );
        assert_eq!(row(Refusal, false), (Rung0::None, 0, true, true, true));
        assert_eq!(row(Safety, false), (Rung0::None, 0, true, true, true));
        assert_eq!(
            row(MalformedToolCall, false),
            (Rung0::CorrectMalformed, 2, true, true, false)
        );
        assert_eq!(
            row(EmptyReply, false),
            (Rung0::NudgeEmpty, 1, true, true, false)
        );
        assert_eq!(
            row(Incomplete, false),
            (Rung0::RetryTurn, 1, true, true, true)
        );
        assert_eq!(
            row(UnrecognizedStop, false),
            (Rung0::None, 0, false, false, false)
        );
        assert_eq!(
            row(UnreportedStop, false),
            (Rung0::None, 0, false, false, false)
        );
        for e in [
            ErrorClass::RateLimited,
            ErrorClass::Overloaded,
            ErrorClass::Server,
            ErrorClass::Timeout,
            ErrorClass::ContextOverflow,
        ] {
            assert_eq!(
                row(ProviderError(e), false),
                (Rung0::ExistingErrorPipeline, 0, false, true, false),
                "{e:?}"
            );
        }
        assert_eq!(
            row(ProviderError(ErrorClass::Quota), false),
            (Rung0::None, 0, false, true, false)
        );
        for e in [ErrorClass::NotFound, ErrorClass::Policy] {
            assert_eq!(
                row(ProviderError(e), false),
                (Rung0::None, 0, true, true, false),
                "{e:?}"
            );
        }
        for e in [
            ErrorClass::Auth,
            ErrorClass::Permission,
            ErrorClass::InvalidRequest,
            ErrorClass::TooLarge,
            ErrorClass::Unrecognized,
        ] {
            assert_eq!(
                row(ProviderError(e), false),
                (Rung0::None, 0, false, false, false),
                "{e:?}"
            );
        }
    }

    #[test]
    fn truncated_tool_only_matters_for_max_tokens() {
        assert_eq!(
            policy(&OutcomeClass::EmptyReply, true),
            policy(&OutcomeClass::EmptyReply, false)
        );
    }

    #[test]
    fn try_budget_counts_per_turn() {
        let mut s = RecoveryState::new();
        let k = Rung0::ContinueTruncated;
        assert_eq!(s.try_budget(4, k, TRUNCATION_BUDGET), Some(1));
        assert_eq!(s.try_budget(4, k, TRUNCATION_BUDGET), Some(2));
        assert_eq!(s.try_budget(4, k, TRUNCATION_BUDGET), Some(3));
        assert_eq!(s.try_budget(4, k, TRUNCATION_BUDGET), None);
        assert_eq!(s.try_budget(5, k, TRUNCATION_BUDGET), Some(1));
        // A different kind on the same turn has its own count.
        assert_eq!(s.try_budget(4, Rung0::NudgeEmpty, 1), Some(1));
    }

    #[test]
    fn take_action_stops_after_the_cap() {
        let mut s = RecoveryState::new();
        for _ in 0..MAX_RECOVERY_ACTIONS {
            assert!(s.take_action());
        }
        assert!(!s.take_action());
    }

    #[test]
    fn next_hop_orders_rung1_then_rung2_and_marks_tried() {
        let chain = vec![
            entry(None, "claude-opus-4-8"),
            entry(Some("codex"), "gpt"),
            entry(Some("anthropic"), "claude-sonnet"),
        ];
        let mut s = RecoveryState::new();
        let hop =
            |s: &mut RecoveryState| s.next_hop(&chain, "anthropic", "claude-opus-5-5", true, true);
        assert_eq!(
            hop(&mut s),
            Some((1, entry(Some("anthropic"), "claude-opus-4-8")))
        );
        assert_eq!(hop(&mut s), Some((1, chain[2].clone())));
        assert_eq!(hop(&mut s), Some((2, chain[1].clone())));
        assert_eq!(hop(&mut s), None);
    }

    #[test]
    fn next_hop_respects_rung_flags() {
        let chain = vec![entry(None, "claude-opus-4-8"), entry(Some("codex"), "gpt")];
        let mut s = RecoveryState::new();
        assert_eq!(
            s.next_hop(&chain, "anthropic", "claude-opus-5-5", false, true),
            Some((2, chain[1].clone()))
        );
        assert_eq!(
            s.next_hop(&chain, "anthropic", "claude-opus-5-5", false, true),
            None
        );
        let mut s = RecoveryState::new();
        assert_eq!(
            s.next_hop(&chain, "anthropic", "claude-opus-5-5", true, false),
            Some((1, entry(Some("anthropic"), "claude-opus-4-8")))
        );
        assert_eq!(
            s.next_hop(&chain, "anthropic", "claude-opus-5-5", true, false),
            None
        );
    }

    #[test]
    fn next_hop_skips_the_current_model() {
        let chain = vec![
            entry(None, "claude-opus-5-5"),
            entry(Some("anthropic"), "claude-opus-5-5"),
            entry(Some("codex"), "gpt"),
        ];
        let mut s = RecoveryState::new();
        assert_eq!(
            s.next_hop(&chain, "anthropic", "claude-opus-5-5", true, true),
            Some((2, chain[2].clone()))
        );
    }

    /// After a hop to another provider, an unnamed entry still means the
    /// origin provider (spec §6.1) and is still rung 1.
    #[test]
    fn unnamed_entries_resolve_against_the_origin_after_a_hop() {
        let chain = vec![
            entry(None, "claude-a"),
            entry(Some("codex"), "gpt-x"),
            entry(Some("gemini"), "gemini-y"),
        ];
        let mut s = RecoveryState::with_origin("anthropic", "claude-opus-5-5");
        // On the origin, overloaded: rung 2 only.
        assert_eq!(
            s.next_hop(&chain, "anthropic", "claude-opus-5-5", false, true),
            Some((2, chain[1].clone()))
        );
        // Now on codex, refused: the unnamed entry is anthropic's, rung 1.
        assert_eq!(
            s.next_hop(&chain, "codex", "gpt-x", true, true),
            Some((1, entry(Some("anthropic"), "claude-a")))
        );
        assert_eq!(
            s.next_hop(&chain, "codex", "gpt-x", true, true),
            Some((2, chain[2].clone()))
        );
        assert_eq!(s.next_hop(&chain, "codex", "gpt-x", true, true), None);
    }

    /// Neither the current pair nor the origin pair (already run) is a hop.
    #[test]
    fn next_hop_never_returns_to_the_origin_model() {
        let chain = vec![
            entry(Some("anthropic"), "claude-opus-5-5"),
            entry(Some("gemini"), "gemini-y"),
        ];
        let mut s = RecoveryState::with_origin("anthropic", "claude-opus-5-5");
        assert_eq!(
            s.next_hop(&chain, "codex", "gpt-x", true, true),
            Some((2, chain[1].clone()))
        );
        assert_eq!(s.next_hop(&chain, "gemini", "gemini-y", true, true), None);
    }

    /// At the action cap no entry is selected, so none is marked tried.
    #[test]
    fn take_hop_at_the_cap_marks_nothing_tried() {
        let chain = vec![entry(None, "claude-opus-4-8")];
        let mut s = RecoveryState::with_origin("anthropic", "claude-opus-5-5");
        while s.take_action() {}
        assert!(!s.can_act());
        assert_eq!(
            s.take_hop(&chain, "anthropic", "claude-opus-5-5", true, true),
            None
        );
        assert_eq!(
            s.next_hop(&chain, "anthropic", "claude-opus-5-5", true, true),
            Some((1, entry(Some("anthropic"), "claude-opus-4-8"))),
            "the capped call left the entry untried"
        );
    }

    #[test]
    fn take_hop_charges_one_action() {
        let chain = vec![entry(None, "claude-opus-4-8")];
        let mut s = RecoveryState::with_origin("anthropic", "claude-opus-5-5");
        for _ in 1..MAX_RECOVERY_ACTIONS {
            assert!(s.take_action());
        }
        assert!(s
            .take_hop(&chain, "anthropic", "claude-opus-5-5", true, true)
            .is_some());
        assert!(!s.can_act());
    }

    #[test]
    fn malformed_note_matches_the_contract() {
        assert_eq!(
            malformed_note("read_file", "EOF"),
            "Your last tool call could not be used: read_file: EOF. Call the tool again with valid arguments."
        );
    }

    #[test]
    fn recovery_retry_note_fills_placeholders() {
        assert_eq!(
            recovery_retry_note("Reply refused", "codex", "gpt"),
            "A previous attempt at this task stopped (Reply refused). You are continuing on codex/gpt; check the current state, then continue the task."
        );
    }

    #[test]
    fn outcome_ids_count_from_one() {
        let mut s = RecoveryState::new();
        assert_eq!(s.next_outcome_id(), "oc_1");
        assert_eq!(s.next_outcome_id(), "oc_2");
    }
}

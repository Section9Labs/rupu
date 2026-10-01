//! Resolved per-run model limits (spec
//! `docs/superpowers/specs/2026-09-30-rupu-model-limits-discovery-design.md` §5).
//! Types and pure math only; resolution lives in `rupu_runtime::model_limits`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Anthropic requires `max_tokens`. When the output cap is neither pinned nor
/// discovered, this is what goes on the wire (spec §6.3).
pub const ANTHROPIC_FALLBACK_MAX_TOKENS: u32 = 8192;

/// Ceiling on the `max_tokens` of a non-streaming request. A non-streaming
/// response is one HTTP exchange, so a discovered 64K–128K output cap would let
/// a single generation outlive the HTTP total timeout — and the resulting error
/// is retryable, so it would be re-sent and re-billed. An agent `maxTokens` pin
/// is exempt: it is the operator's explicit choice.
pub const NON_STREAMING_MAX_TOKENS: u32 = 16_384;

/// Compaction percentage when the agent doesn't set `compactAtPercent`.
pub const DEFAULT_COMPACT_AT_PERCENT: u8 = 80;

/// Where a resolved limit came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LimitSource {
    /// Agent frontmatter (`contextWindowTokens` / `maxTokens`).
    Agent,
    /// `[[providers.<name>.models]]` in config.
    Config,
    /// The provider's model-list endpoint, via the 1h cache.
    Live {
        fetched_at: DateTime<Utc>,
        stale: bool,
    },
    /// Learned from a provider overflow error during this run (spec §7).
    Observed,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Limit {
    pub tokens: Option<u32>,
    pub source: LimitSource,
}

impl Limit {
    pub fn unknown() -> Self {
        Self {
            tokens: None,
            source: LimitSource::Unknown,
        }
    }
    pub fn new(tokens: u32, source: LimitSource) -> Self {
        Self {
            tokens: Some(tokens),
            source,
        }
    }
}

/// The limits one run uses. `input` is the number of input tokens the model
/// accepts (spec §3 semantics); `output` is the max output tokens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelLimits {
    pub input: Limit,
    pub output: Limit,
    /// Agent value, else [`DEFAULT_COMPACT_AT_PERCENT`]; clamped to `[10, 95]`.
    pub compact_at_percent: u8,
    /// Whether output counts against the same window as input
    /// (`LlmProvider::output_shares_context`). Drives the headroom rule.
    pub output_shares_context: bool,
    /// What the provider sends when `output` is unknown (Anthropic:
    /// `Some(8192)`; providers where the cap is optional: `None`, meaning
    /// omitted). Display only; the provider applies it.
    #[serde(default)]
    pub output_fallback: Option<u32>,
    /// Why a limit is unknown or stale, appended to the notice.
    #[serde(default)]
    pub note: Option<String>,
}

impl ModelLimits {
    pub fn unknown() -> Self {
        Self {
            input: Limit::unknown(),
            output: Limit::unknown(),
            compact_at_percent: DEFAULT_COMPACT_AT_PERCENT,
            output_shares_context: true,
            output_fallback: None,
            note: None,
        }
    }

    /// Both limits pinned (source `Agent`). Tests and fixed-budget callers.
    pub fn fixed(input: u32, output: u32) -> Self {
        Self::unknown().with_input(input).with_output(output)
    }

    /// Agent-frontmatter pins only, with no discovery. Every unset pin stays unknown.
    pub fn from_pins(input: Option<u32>, output: Option<u32>, pct: Option<u8>) -> Self {
        let mut l = Self::unknown();
        if let Some(n) = input {
            l = l.with_input(n);
        }
        if let Some(n) = output {
            l = l.with_output(n);
        }
        if let Some(p) = pct {
            l = l.with_percent(p);
        }
        l
    }

    pub fn with_input(mut self, n: u32) -> Self {
        self.input = Limit::new(n, LimitSource::Agent);
        self
    }
    pub fn with_output(mut self, n: u32) -> Self {
        self.output = Limit::new(n, LimitSource::Agent);
        self
    }
    pub fn with_percent(mut self, pct: u8) -> Self {
        self.compact_at_percent = pct.clamp(10, 95);
        self
    }

    fn by_percent(&self) -> Option<u64> {
        let input = self.input.tokens? as u64;
        Some(input * self.compact_at_percent.clamp(10, 95) as u64 / 100)
    }

    /// Input-token count above which proactive compaction runs (spec §6.4):
    /// `min(input × pct, input − output)` when output shares the window,
    /// else `input × pct`; `None` when the input limit is unknown.
    pub fn compact_threshold(&self) -> Option<u64> {
        let by_pct = self.by_percent()?;
        let input = self.input.tokens? as u64;
        let headroom = match (self.output.tokens, self.output_shares_context) {
            (Some(out), true) => input.checked_sub(out as u64).filter(|h| *h > 0),
            _ => None,
        };
        Some(headroom.map_or(by_pct, |h| by_pct.min(h)))
    }

    /// The `max_tokens` a request should carry: the resolved output limit,
    /// clamped to [`NON_STREAMING_MAX_TOKENS`] for a non-streaming request
    /// unless the limit is an agent pin. `None` when the output is unknown (the
    /// provider's own fallback applies).
    pub fn request_max_tokens(&self, streaming: bool) -> Option<u32> {
        let known = self.output.tokens?;
        if streaming || self.output.source == LimitSource::Agent {
            Some(known)
        } else {
            Some(known.min(NON_STREAMING_MAX_TOKENS))
        }
    }

    /// Whether a non-streaming request would send a smaller `max_tokens` than
    /// the resolved output limit (what [`Self::request_max_tokens`] clamps).
    pub fn non_streaming_cap_applies(&self) -> bool {
        self.request_max_tokens(false) != self.output.tokens
    }

    /// Neither limit has a source: nothing was pinned, configured, or
    /// discovered. A session that stored such a value (a transient first-turn
    /// failure) should resolve again rather than keep it.
    pub fn is_unresolved(&self) -> bool {
        self.input.source == LimitSource::Unknown && self.output.source == LimitSource::Unknown
    }

    /// Lower the input limit to `max` (source `Observed`) when `max` is below
    /// the current value or the current value is unknown. Never raises it.
    /// Returns `true` when the limit changed.
    pub fn clamp_input(&mut self, max: u32) -> bool {
        match self.input.tokens {
            Some(cur) if cur <= max => false,
            _ => {
                self.input = Limit::new(max, LimitSource::Observed);
                true
            }
        }
    }

    /// The run-start `Notice { kind: "model_limits" }` text (spec §6.6).
    pub fn describe(&self, provider_name: &str, now: DateTime<Utc>) -> String {
        let input = match self.input.tokens {
            Some(n) => format!("input {}", group_thousands(n as u64)),
            None => "input unknown".to_string(),
        };
        let output = match (self.output.tokens, self.output_fallback) {
            (Some(n), _) => format!("output {}", group_thousands(n as u64)),
            (None, Some(f)) => format!("output unknown ({f} fallback)"),
            (None, None) => "output unknown (provider max)".to_string(),
        };
        let compact = match (self.compact_threshold(), self.by_percent()) {
            (Some(t), Some(p)) if t < p => {
                format!("compact at {} (output headroom)", group_thousands(t))
            }
            (Some(t), _) => format!(
                "compact at {} ({}%)",
                group_thousands(t),
                self.compact_at_percent
            ),
            (None, _) => "compaction off".to_string(),
        };
        let mut origins: Vec<String> = Vec::new();
        for src in [&self.input.source, &self.output.source] {
            let text = match src {
                LimitSource::Agent => "agent frontmatter".to_string(),
                LimitSource::Config => format!("[providers.{provider_name}.models]"),
                LimitSource::Live { fetched_at, stale } => format!(
                    "{provider_name} model list, cached {}{}",
                    fmt_age(now - *fetched_at),
                    if *stale { " (stale)" } else { "" }
                ),
                LimitSource::Observed => "provider error".to_string(),
                LimitSource::Unknown => continue,
            };
            if !origins.contains(&text) {
                origins.push(text);
            }
        }
        let origin = if origins.is_empty() {
            "no limit source".to_string()
        } else {
            origins.join(" + ")
        };
        let mut s = format!("{input} · {output} · {compact} — {origin}");
        if let Some(note) = &self.note {
            s.push_str("; ");
            s.push_str(note);
        }
        s
    }
}

/// `1048576` → `"1,048,576"`.
pub fn group_thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// `just now` / `12m ago` / `3h ago` / `2d ago`.
pub fn fmt_age(age: chrono::Duration) -> String {
    let mins = age.num_minutes();
    if mins < 1 {
        "just now".to_string()
    } else if mins < 60 {
        format!("{mins}m ago")
    } else if mins < 60 * 24 {
        format!("{}h ago", mins / 60)
    } else {
        format!("{}d ago", mins / (60 * 24))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(h: u32, m: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 30, h, m, 0).unwrap()
    }

    #[test]
    fn threshold_is_off_when_input_unknown() {
        assert_eq!(ModelLimits::unknown().compact_threshold(), None);
    }

    #[test]
    fn threshold_uses_percent_when_output_unknown() {
        let l = ModelLimits::unknown().with_input(1_000_000);
        assert_eq!(l.compact_threshold(), Some(800_000));
    }

    #[test]
    fn headroom_rule_wins_for_haiku_sized_models() {
        // 200K input / 64K output: 80% = 160K, headroom = 136K (spec §6.4)
        let l = ModelLimits::fixed(200_000, 64_000);
        assert_eq!(l.compact_threshold(), Some(136_000));
    }

    #[test]
    fn percent_wins_for_1m_models() {
        let l = ModelLimits::fixed(1_000_000, 128_000);
        assert_eq!(l.compact_threshold(), Some(800_000));
    }

    #[test]
    fn headroom_rule_is_skipped_when_output_does_not_share_the_window() {
        let mut l = ModelLimits::fixed(128_000, 128_000);
        l.output_shares_context = false;
        assert_eq!(l.compact_threshold(), Some(102_400));
    }

    #[test]
    fn headroom_is_ignored_when_output_is_not_smaller_than_input() {
        let l = ModelLimits::fixed(8_000, 8_000);
        assert_eq!(l.compact_threshold(), Some(6_400));
    }

    #[test]
    fn percent_is_clamped() {
        let l = ModelLimits::unknown().with_input(1000).with_percent(99);
        assert_eq!(l.compact_at_percent, 95);
        assert_eq!(l.compact_threshold(), Some(950));
    }

    #[test]
    fn clamp_input_lowers_only() {
        let mut l = ModelLimits::unknown().with_input(1_000_000);
        assert!(l.clamp_input(200_000));
        assert_eq!(l.input, Limit::new(200_000, LimitSource::Observed));
        assert!(!l.clamp_input(500_000), "never raises");
        let mut u = ModelLimits::unknown();
        assert!(
            u.clamp_input(128_000),
            "unknown input accepts the observed value"
        );
    }

    #[test]
    fn from_pins_marks_agent_source() {
        let l = ModelLimits::from_pins(Some(1000), None, Some(50));
        assert_eq!(l.input, Limit::new(1000, LimitSource::Agent));
        assert_eq!(l.output, Limit::unknown());
        assert_eq!(l.compact_at_percent, 50);
    }

    #[test]
    fn describe_known_live_limits() {
        let mut l = ModelLimits::unknown();
        let fetched = at(12, 0);
        l.input = Limit::new(
            1_000_000,
            LimitSource::Live {
                fetched_at: fetched,
                stale: false,
            },
        );
        l.output = Limit::new(
            128_000,
            LimitSource::Live {
                fetched_at: fetched,
                stale: false,
            },
        );
        assert_eq!(
			l.describe("anthropic", at(12, 12)),
			"input 1,000,000 · output 128,000 · compact at 800,000 (80%) — anthropic model list, cached 12m ago"
		);
    }

    #[test]
    fn describe_unknown_with_fallback_and_note() {
        let mut l = ModelLimits::unknown();
        l.output_fallback = Some(ANTHROPIC_FALLBACK_MAX_TOKENS);
        l.note = Some("gemini exposes no model limits".into());
        assert_eq!(
			l.describe("gemini", at(12, 0)),
			"input unknown · output unknown (8192 fallback) · compaction off — no limit source; gemini exposes no model limits"
		);
    }

    #[test]
    fn describe_marks_headroom_and_stale() {
        let fetched = at(9, 0);
        let mut l = ModelLimits::unknown();
        l.input = Limit::new(
            200_000,
            LimitSource::Live {
                fetched_at: fetched,
                stale: true,
            },
        );
        l.output = Limit::new(64_000, LimitSource::Agent);
        assert_eq!(
			l.describe("anthropic", at(12, 0)),
			"input 200,000 · output 64,000 · compact at 136,000 (output headroom) — anthropic model list, cached 3h ago (stale) + agent frontmatter"
		);
    }

    #[test]
    fn group_thousands_formats() {
        assert_eq!(group_thousands(0), "0");
        assert_eq!(group_thousands(999), "999");
        assert_eq!(group_thousands(1_048_576), "1,048,576");
    }

    #[test]
    fn non_streaming_cap_clamps_a_discovered_output_only() {
        let live = LimitSource::Live {
            fetched_at: at(12, 0),
            stale: false,
        };
        let mut l = ModelLimits::unknown();
        l.output = Limit::new(128_000, live.clone());
        assert_eq!(
            l.request_max_tokens(true),
            Some(128_000),
            "streaming: as-is"
        );
        assert_eq!(
            l.request_max_tokens(false),
            Some(NON_STREAMING_MAX_TOKENS),
            "non-streaming: clamped"
        );
        assert!(l.non_streaming_cap_applies());

        l.output = Limit::new(128_000, LimitSource::Config);
        assert_eq!(l.request_max_tokens(false), Some(NON_STREAMING_MAX_TOKENS));

        l.output = Limit::new(8_192, live);
        assert_eq!(l.request_max_tokens(false), Some(8_192), "already below");
        assert!(!l.non_streaming_cap_applies());

        l.output = Limit::new(128_000, LimitSource::Agent);
        assert_eq!(l.request_max_tokens(false), Some(128_000), "pin honoured");
        assert!(!l.non_streaming_cap_applies());

        assert_eq!(ModelLimits::unknown().request_max_tokens(false), None);
        assert!(!ModelLimits::unknown().non_streaming_cap_applies());
    }

    #[test]
    fn is_unresolved_only_when_both_sources_are_unknown() {
        assert!(ModelLimits::unknown().is_unresolved());
        assert!(!ModelLimits::unknown().with_input(1000).is_unresolved());
        assert!(!ModelLimits::unknown().with_output(1000).is_unresolved());
        let mut observed = ModelLimits::unknown();
        observed.clamp_input(1000);
        assert!(!observed.is_unresolved());
        // A note or fallback alone does not make it resolved.
        let mut noted = ModelLimits::unknown();
        noted.note = Some("refresh failed".into());
        noted.output_fallback = Some(ANTHROPIC_FALLBACK_MAX_TOKENS);
        assert!(noted.is_unresolved());
    }

    #[test]
    fn serde_round_trips() {
        let l = ModelLimits::fixed(10, 5).with_percent(60);
        let s = serde_json::to_string(&l).unwrap();
        let back: ModelLimits = serde_json::from_str(&s).unwrap();
        assert_eq!(back, l);
    }
}

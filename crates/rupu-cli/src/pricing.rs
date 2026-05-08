//! USD price lookup for token usage.
//!
//! Three tiers, resolved in this order:
//!
//! 1. **User config** — `[pricing.<provider>."<model>"]` in
//!    `~/.rupu/config.toml` or `<repo>/.rupu/config.toml`.
//! 2. **Built-in defaults** — [`BUILTIN_PRICES`] below, updated as
//!    public vendor pricing changes. Acts as a sane out-of-the-box
//!    fallback so `rupu usage` reports cost without configuration.
//! 3. **Per-agent fallback** — `[pricing.agents.<agent-name>]`. The
//!    hatch the user opens for private / internal endpoints whose
//!    `(provider, model)` pair has no public price.
//!
//! Vendor pricing changes; treat the built-in table as a default and
//! override in config when accuracy matters. Provider keys match the
//! `ProviderId::auth_key()` strings the agent runtime stamps onto
//! `Event::RunStart` (`anthropic`, `openai-codex`, `google-gemini-cli`,
//! `google-antigravity`, `github-copilot`).

use rupu_config::{ModelPricing, PricingConfig};

/// Built-in USD-per-million-tokens defaults for the major models.
/// Last reviewed: 2026-05-07. Pricing drifts over time — users with
/// strict cost reporting needs should override in config.
///
/// Provider keys use the canonical `ProviderId::auth_key()` strings;
/// [`canonicalize_provider`] maps the friendly aliases users actually
/// write in agent frontmatter (`openai`, `gemini`, `copilot`, …) onto
/// these canonical keys before lookup.
pub const BUILTIN_PRICES: &[(&str, &str, ModelPricing)] = &[
    // ── Anthropic ─────────────────────────────────────────────────
    (
        "anthropic",
        "claude-opus-4-7",
        ModelPricing {
            input_per_mtok: 15.0,
            output_per_mtok: 75.0,
            cached_input_per_mtok: Some(1.50),
        },
    ),
    (
        "anthropic",
        "claude-sonnet-4-6",
        ModelPricing {
            input_per_mtok: 3.0,
            output_per_mtok: 15.0,
            cached_input_per_mtok: Some(0.30),
        },
    ),
    (
        "anthropic",
        "claude-haiku-4-5",
        ModelPricing {
            input_per_mtok: 0.80,
            output_per_mtok: 4.0,
            cached_input_per_mtok: Some(0.08),
        },
    ),
    // ── OpenAI ────────────────────────────────────────────────────
    (
        "openai-codex",
        "gpt-5",
        ModelPricing {
            input_per_mtok: 1.25,
            output_per_mtok: 10.0,
            cached_input_per_mtok: Some(0.125),
        },
    ),
    (
        "openai-codex",
        "gpt-5-mini",
        ModelPricing {
            input_per_mtok: 0.25,
            output_per_mtok: 2.0,
            cached_input_per_mtok: Some(0.025),
        },
    ),
    (
        "openai-codex",
        "gpt-4o",
        ModelPricing {
            input_per_mtok: 2.50,
            output_per_mtok: 10.0,
            cached_input_per_mtok: Some(1.25),
        },
    ),
    // ── Google Gemini ─────────────────────────────────────────────
    (
        "google-gemini-cli",
        "gemini-2.5-pro",
        ModelPricing {
            input_per_mtok: 1.25,
            output_per_mtok: 10.0,
            cached_input_per_mtok: Some(0.31),
        },
    ),
    (
        "google-gemini-cli",
        "gemini-2.5-flash",
        ModelPricing {
            input_per_mtok: 0.30,
            output_per_mtok: 2.50,
            cached_input_per_mtok: Some(0.075),
        },
    ),
];

/// Map a user-written provider name onto the canonical `ProviderId`
/// auth key. Agents in the wild say `openai` / `gemini` / `copilot`,
/// but `ProviderId::auth_key()` returns `openai-codex` /
/// `google-gemini-cli` / `github-copilot`. The built-in table uses the
/// canonical form, so we normalize at lookup.
fn canonicalize_provider(provider: &str) -> &str {
    match provider {
        "openai" => "openai-codex",
        "gemini" | "gemini-cli" => "google-gemini-cli",
        "antigravity" => "google-antigravity",
        "copilot" => "github-copilot",
        other => other,
    }
}

/// Strip a trailing `-YYYY-MM-DD` date snapshot from a model id.
/// OpenAI returns dated variants like `gpt-5-2025-08-07` from
/// `/v1/chat/completions`; users configure `gpt-5` in agents and the
/// built-in price table keys on the bare name. Returns the original
/// string when no date suffix matches.
fn strip_date_suffix(model: &str) -> &str {
    // Look for the rightmost `-YYYY-MM-DD` (`-` + 4 digits + `-` + 2 digits + `-` + 2 digits).
    let bytes = model.as_bytes();
    if bytes.len() < 11 {
        return model;
    }
    let tail = &bytes[bytes.len() - 11..];
    let is_date_suffix = tail[0] == b'-'
        && tail[1..5].iter().all(|c| c.is_ascii_digit())
        && tail[5] == b'-'
        && tail[6..8].iter().all(|c| c.is_ascii_digit())
        && tail[8] == b'-'
        && tail[9..11].iter().all(|c| c.is_ascii_digit());
    if is_date_suffix {
        &model[..model.len() - 11]
    } else {
        model
    }
}

/// Resolve a USD price for one `(provider, model, agent)` triple.
///
/// Lookup order — first non-`None` wins:
///
/// 1. `cfg.models[provider][model]` (user-configured, exact match)
/// 2. `cfg.models[provider][stripped]` if the model carries a
///    `[<suffix>]` tag (e.g. `claude-sonnet-4-6[1m]`); we strip the
///    suffix and retry so a single price entry covers both base and
///    extended-context variants.
/// 3. [`BUILTIN_PRICES`] for `(provider, model)` (then `(provider, stripped)`)
/// 4. `cfg.agents[agent]` (user-configured agent-level fallback)
///
/// Returns `None` when no tier yields a price; callers render that as
/// a placeholder (`—`) in the cost column.
pub fn lookup(
    cfg: &PricingConfig,
    provider: &str,
    model: &str,
    agent: &str,
) -> Option<ModelPricing> {
    let canon_provider = canonicalize_provider(provider);
    let no_tag = strip_model_tag(model);
    let no_date = strip_date_suffix(no_tag);

    // Build the candidate model strings to try, in priority order.
    // Most-specific first so an exact configured price always wins
    // over a date-stripped fallback. Dedup so we don't re-query the
    // same key when the model has no date or tag suffix.
    let mut candidates: Vec<&str> = vec![model];
    if no_tag != model {
        candidates.push(no_tag);
    }
    if no_date != no_tag && no_date != model {
        candidates.push(no_date);
    }

    // Tier 1: user-configured. Try the original provider string first
    // (lets users target an alias they wrote in their agents) and
    // then the canonical form, against each candidate model name.
    for prov in [provider, canon_provider].iter().copied() {
        if let Some(per_provider) = cfg.models.get(prov) {
            for cand in &candidates {
                if let Some(p) = per_provider.get(*cand) {
                    return Some(*p);
                }
            }
        }
        // Stop after one if alias and canonical are identical.
        if prov == canon_provider {
            break;
        }
    }

    // Tier 2: built-in table — keyed on the canonical provider.
    for cand in &candidates {
        if let Some(p) = builtin_lookup(canon_provider, cand) {
            return Some(p);
        }
    }

    // Tier 3: user-configured agent-level fallback.
    cfg.agents.get(agent).copied()
}

/// Strip a trailing `[…]` tag from a model id. Used to collapse
/// context-extended variants (`claude-sonnet-4-6[1m]`) onto the base
/// model's price entry.
fn strip_model_tag(model: &str) -> &str {
    match model.rfind('[') {
        Some(i) if model.ends_with(']') => &model[..i],
        _ => model,
    }
}

fn builtin_lookup(provider: &str, model: &str) -> Option<ModelPricing> {
    BUILTIN_PRICES
        .iter()
        .find(|(p, m, _)| *p == provider && *m == model)
        .map(|(_, _, price)| *price)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn empty_cfg() -> PricingConfig {
        PricingConfig::default()
    }

    #[test]
    fn user_config_wins_over_builtin() {
        let mut cfg = empty_cfg();
        let mut anthro = BTreeMap::new();
        anthro.insert(
            "claude-sonnet-4-6".into(),
            ModelPricing {
                input_per_mtok: 99.0, // intentionally wrong vs. builtin
                output_per_mtok: 99.0,
                cached_input_per_mtok: None,
            },
        );
        cfg.models.insert("anthropic".into(), anthro);

        let p = lookup(&cfg, "anthropic", "claude-sonnet-4-6", "any-agent").unwrap();
        assert_eq!(p.input_per_mtok, 99.0);
    }

    #[test]
    fn falls_through_to_builtin_when_no_user_entry() {
        let cfg = empty_cfg();
        let p = lookup(&cfg, "anthropic", "claude-sonnet-4-6", "any-agent").unwrap();
        assert_eq!(p.input_per_mtok, 3.0);
        assert_eq!(p.output_per_mtok, 15.0);
        assert_eq!(p.cached_input_per_mtok, Some(0.30));
    }

    #[test]
    fn falls_through_to_agent_when_no_model_match() {
        let mut cfg = empty_cfg();
        cfg.agents.insert(
            "private-reviewer".into(),
            ModelPricing {
                input_per_mtok: 5.0,
                output_per_mtok: 25.0,
                cached_input_per_mtok: None,
            },
        );
        // Provider+model that nobody knows about → should hit agent rung.
        let p = lookup(&cfg, "internal-vllm", "llama-3-70b", "private-reviewer").unwrap();
        assert_eq!(p.input_per_mtok, 5.0);
    }

    #[test]
    fn returns_none_when_nothing_matches() {
        let cfg = empty_cfg();
        assert!(lookup(&cfg, "fake-provider", "fake-model", "fake-agent").is_none());
    }

    #[test]
    fn strips_context_tag_for_lookup() {
        // 1M-context variant should resolve via the base entry.
        let cfg = empty_cfg();
        let p = lookup(&cfg, "anthropic", "claude-sonnet-4-6[1m]", "any").unwrap();
        assert_eq!(p.input_per_mtok, 3.0);
    }

    #[test]
    fn user_exact_wins_over_user_stripped() {
        // If a user configures BOTH the suffixed and the bare name,
        // exact wins — they explicitly priced the long-context tier.
        let mut cfg = empty_cfg();
        let mut anthro = BTreeMap::new();
        anthro.insert(
            "claude-sonnet-4-6".into(),
            ModelPricing {
                input_per_mtok: 3.0,
                output_per_mtok: 15.0,
                cached_input_per_mtok: None,
            },
        );
        anthro.insert(
            "claude-sonnet-4-6[1m]".into(),
            ModelPricing {
                input_per_mtok: 6.0,
                output_per_mtok: 22.5,
                cached_input_per_mtok: None,
            },
        );
        cfg.models.insert("anthropic".into(), anthro);

        let p = lookup(&cfg, "anthropic", "claude-sonnet-4-6[1m]", "any").unwrap();
        assert_eq!(p.input_per_mtok, 6.0);
    }

    #[test]
    fn strip_model_tag_handles_no_tag() {
        assert_eq!(strip_model_tag("gpt-5"), "gpt-5");
    }

    #[test]
    fn strip_model_tag_only_strips_trailing_bracket() {
        // A bracket mid-string (unusual) shouldn't be misread.
        assert_eq!(strip_model_tag("foo[bar]baz"), "foo[bar]baz");
    }

    #[test]
    fn provider_alias_canonicalized_for_builtin_lookup() {
        // Agent files write `provider: openai`, but the built-in
        // table keys on `openai-codex`. The alias should resolve.
        let cfg = empty_cfg();
        let p = lookup(&cfg, "openai", "gpt-5", "any-agent").unwrap();
        assert_eq!(p.input_per_mtok, 1.25);

        let g = lookup(&cfg, "gemini", "gemini-2.5-pro", "any-agent").unwrap();
        assert_eq!(g.input_per_mtok, 1.25);
    }

    #[test]
    fn dated_openai_model_resolves_to_base_price() {
        // OpenAI returns versioned model IDs in usage events.
        // `gpt-5-2025-08-07` should fall back to `gpt-5`.
        let cfg = empty_cfg();
        let p = lookup(&cfg, "openai", "gpt-5-2025-08-07", "any-agent").unwrap();
        assert_eq!(p.input_per_mtok, 1.25);
    }

    #[test]
    fn dated_anthropic_model_resolves_to_base_price() {
        let cfg = empty_cfg();
        let p = lookup(&cfg, "anthropic", "claude-sonnet-4-6-2026-01-15", "any").unwrap();
        assert_eq!(p.input_per_mtok, 3.0);
    }

    #[test]
    fn user_alias_provider_entry_wins_over_builtin() {
        // User wrote `[pricing.openai."gpt-5"]` in config — that
        // should win over the built-in `openai-codex.gpt-5` entry.
        let mut cfg = empty_cfg();
        let mut openai_user = BTreeMap::new();
        openai_user.insert(
            "gpt-5".into(),
            ModelPricing {
                input_per_mtok: 99.0,
                output_per_mtok: 99.0,
                cached_input_per_mtok: None,
            },
        );
        cfg.models.insert("openai".into(), openai_user);

        let p = lookup(&cfg, "openai", "gpt-5", "any").unwrap();
        assert_eq!(p.input_per_mtok, 99.0);
    }

    #[test]
    fn strip_date_suffix_handles_no_date() {
        assert_eq!(strip_date_suffix("gpt-5"), "gpt-5");
        assert_eq!(strip_date_suffix("claude-sonnet-4-6"), "claude-sonnet-4-6");
        // Wrong-shape suffix: "-2025-X-07" → not a date, leave alone.
        assert_eq!(strip_date_suffix("gpt-5-2025-aa-07"), "gpt-5-2025-aa-07");
    }

    #[test]
    fn strip_date_suffix_strips_trailing_iso_date() {
        assert_eq!(strip_date_suffix("gpt-5-2025-08-07"), "gpt-5");
        assert_eq!(strip_date_suffix("gpt-4o-mini-2024-07-18"), "gpt-4o-mini");
    }
}

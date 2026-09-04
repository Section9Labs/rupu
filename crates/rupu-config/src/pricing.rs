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

use crate::{ModelPricing, PricingConfig};

/// Built-in USD-per-million-tokens defaults for the major models.
/// Last reviewed: 2026-09-04, all three sections, against
/// <https://developers.openai.com/api/docs/pricing>,
/// <https://platform.claude.com/docs/en/about-claude/pricing>, and
/// <https://ai.google.dev/gemini-api/docs/pricing>. Pricing drifts over
/// time — users with strict cost reporting needs should override in
/// config.
///
/// Provider keys use the canonical `ProviderId::auth_key()` strings;
/// [`canonicalize_provider`] maps the friendly aliases users actually
/// write in agent frontmatter (`openai`, `gemini`, `copilot`, …) onto
/// these canonical keys before lookup.
pub const BUILTIN_PRICES: &[(&str, &str, ModelPricing)] = &[
    // ── Anthropic ─────────────────────────────────────────────────
    // First-party API rates from
    // https://platform.claude.com/docs/en/about-claude/pricing.
    // `cached_input_per_mtok` is the cache-READ (hit) rate. Cache
    // writes bill at 1.25x (5m) / 2x (1h) the input rate; transcripts
    // fold cache-creation tokens into `input_tokens`, so writes are
    // billed here at 1x and slightly under-counted. Long context is
    // included at standard rates on 4.6+ models, so no surcharge to
    // model. Batch discounts and the 1.1x `inference_geo: "us"` uplift
    // are not modeled.
    // Fable 5.1 / Mythos 5.1 cache hits bill at 0.025x input (all other
    // models: 0.1x).
    (
        "anthropic",
        "claude-fable-5-1",
        ModelPricing {
            input_per_mtok: 10.0,
            output_per_mtok: 50.0,
            cached_input_per_mtok: Some(0.25),
        },
    ),
    (
        "anthropic",
        "claude-mythos-5-1",
        ModelPricing {
            input_per_mtok: 10.0,
            output_per_mtok: 50.0,
            cached_input_per_mtok: Some(0.25),
        },
    ),
    (
        "anthropic",
        "claude-fable-5",
        ModelPricing {
            input_per_mtok: 10.0,
            output_per_mtok: 50.0,
            cached_input_per_mtok: Some(1.0),
        },
    ),
    (
        "anthropic",
        "claude-mythos-5",
        ModelPricing {
            input_per_mtok: 10.0,
            output_per_mtok: 50.0,
            cached_input_per_mtok: Some(1.0),
        },
    ),
    (
        "anthropic",
        "claude-opus-5",
        ModelPricing {
            input_per_mtok: 5.0,
            output_per_mtok: 25.0,
            cached_input_per_mtok: Some(0.50),
        },
    ),
    (
        "anthropic",
        "claude-opus-4-8",
        ModelPricing {
            input_per_mtok: 5.0,
            output_per_mtok: 25.0,
            cached_input_per_mtok: Some(0.50),
        },
    ),
    (
        "anthropic",
        "claude-opus-4-7",
        ModelPricing {
            input_per_mtok: 5.0,
            output_per_mtok: 25.0,
            cached_input_per_mtok: Some(0.50),
        },
    ),
    (
        "anthropic",
        "claude-opus-4-6",
        ModelPricing {
            input_per_mtok: 5.0,
            output_per_mtok: 25.0,
            cached_input_per_mtok: Some(0.50),
        },
    ),
    (
        "anthropic",
        "claude-opus-4-5",
        ModelPricing {
            input_per_mtok: 5.0,
            output_per_mtok: 25.0,
            cached_input_per_mtok: Some(0.50),
        },
    ),
    // Retired on the first-party API; still priced for historical runs.
    (
        "anthropic",
        "claude-opus-4-1",
        ModelPricing {
            input_per_mtok: 15.0,
            output_per_mtok: 75.0,
            cached_input_per_mtok: Some(1.50),
        },
    ),
    (
        "anthropic",
        "claude-opus-4",
        ModelPricing {
            input_per_mtok: 15.0,
            output_per_mtok: 75.0,
            cached_input_per_mtok: Some(1.50),
        },
    ),
    (
        "anthropic",
        "claude-sonnet-5",
        ModelPricing {
            input_per_mtok: 2.0,
            output_per_mtok: 10.0,
            cached_input_per_mtok: Some(0.20),
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
        "claude-sonnet-4-5",
        ModelPricing {
            input_per_mtok: 3.0,
            output_per_mtok: 15.0,
            cached_input_per_mtok: Some(0.30),
        },
    ),
    (
        "anthropic",
        "claude-sonnet-4",
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
            input_per_mtok: 1.0,
            output_per_mtok: 5.0,
            cached_input_per_mtok: Some(0.10),
        },
    ),
    // Haiku 3.5 keeps the older `claude-3-5-haiku-<YYYYMMDD>` id shape;
    // the compact date strips off before lookup.
    (
        "anthropic",
        "claude-3-5-haiku",
        ModelPricing {
            input_per_mtok: 0.80,
            output_per_mtok: 4.0,
            cached_input_per_mtok: Some(0.08),
        },
    ),
    // Retro-alias: legacy transcripts recorded Anthropic's served-model id
    // (`claude-mythos-preview`) for EVERY Claude request, collapsing
    // opus/sonnet/haiku into one unpriced line. New runs record the requested
    // model (priced correctly); this entry retro-prices the collapsed historical
    // data at the Opus 4.1-era flagship rate it was first approximated with.
    // Deliberately left at that figure: it is an approximation of historical
    // data, not a vendor price, and moving it would silently rewrite past
    // cost reports.
    (
        "anthropic",
        "claude-mythos-preview",
        ModelPricing {
            input_per_mtok: 15.0,
            output_per_mtok: 75.0,
            cached_input_per_mtok: Some(1.50),
        },
    ),
    // ── OpenAI ────────────────────────────────────────────────────
    // Standard-tier, short-context text rates from
    // https://developers.openai.com/api/docs/pricing. OpenAI doubles
    // input, cached-input, and output rates once a request crosses into
    // its long-context tier; transcripts record token counts but not
    // which tier OpenAI billed, so this table carries the short-context
    // rate and under-bills long-context requests. Batch and Flex
    // discounts and the Fast-mode uplift are likewise not modeled.
    (
        "openai-codex",
        "gpt-6-astra",
        ModelPricing {
            input_per_mtok: 10.0,
            output_per_mtok: 50.0,
            cached_input_per_mtok: Some(1.0),
        },
    ),
    // Promotional rate, published as available at least through 2026-11-21.
    (
        "openai-codex",
        "gpt-5.6-sol",
        ModelPricing {
            input_per_mtok: 4.0,
            output_per_mtok: 20.0,
            cached_input_per_mtok: Some(0.40),
        },
    ),
    (
        "openai-codex",
        "gpt-5.6-terra",
        ModelPricing {
            input_per_mtok: 2.0,
            output_per_mtok: 12.0,
            cached_input_per_mtok: Some(0.20),
        },
    ),
    (
        "openai-codex",
        "gpt-5.6-luna",
        ModelPricing {
            input_per_mtok: 0.20,
            output_per_mtok: 1.20,
            cached_input_per_mtok: Some(0.02),
        },
    ),
    // Daybreak cyber model; short-context tier only is published.
    (
        "openai-codex",
        "gpt-5.6-cyber",
        ModelPricing {
            input_per_mtok: 12.50,
            output_per_mtok: 75.0,
            cached_input_per_mtok: Some(1.25),
        },
    ),
    // Daybreak aliases: `blue` currently points at gpt-5.6-sol and `red` at
    // gpt-5.6-cyber. OpenAI re-points these as new Daybreak models ship, so
    // re-check them whenever the table is reviewed.
    (
        "openai-codex",
        "gpt-daybreak-blue-latest",
        ModelPricing {
            input_per_mtok: 4.0,
            output_per_mtok: 20.0,
            cached_input_per_mtok: Some(0.40),
        },
    ),
    (
        "openai-codex",
        "gpt-daybreak-red-latest",
        ModelPricing {
            input_per_mtok: 12.50,
            output_per_mtok: 75.0,
            cached_input_per_mtok: Some(1.25),
        },
    ),
    (
        "openai-codex",
        "gpt-5.5",
        ModelPricing {
            input_per_mtok: 5.0,
            output_per_mtok: 30.0,
            cached_input_per_mtok: Some(0.50),
        },
    ),
    // Pro-tier models publish no cached-input rate; `None` bills cache hits
    // at the full input rate rather than inventing a discount.
    (
        "openai-codex",
        "gpt-5.5-pro",
        ModelPricing {
            input_per_mtok: 30.0,
            output_per_mtok: 180.0,
            cached_input_per_mtok: None,
        },
    ),
    (
        "openai-codex",
        "gpt-5.4",
        ModelPricing {
            input_per_mtok: 2.50,
            output_per_mtok: 15.0,
            cached_input_per_mtok: Some(0.25),
        },
    ),
    (
        "openai-codex",
        "gpt-5.4-mini",
        ModelPricing {
            input_per_mtok: 0.75,
            output_per_mtok: 4.50,
            cached_input_per_mtok: Some(0.075),
        },
    ),
    (
        "openai-codex",
        "gpt-5.4-nano",
        ModelPricing {
            input_per_mtok: 0.20,
            output_per_mtok: 1.25,
            cached_input_per_mtok: Some(0.02),
        },
    ),
    (
        "openai-codex",
        "gpt-5.4-pro",
        ModelPricing {
            input_per_mtok: 30.0,
            output_per_mtok: 180.0,
            cached_input_per_mtok: None,
        },
    ),
    (
        "openai-codex",
        "gpt-5.3-codex",
        ModelPricing {
            input_per_mtok: 1.75,
            output_per_mtok: 14.0,
            cached_input_per_mtok: Some(0.175),
        },
    ),
    (
        "openai-codex",
        "gpt-5.2",
        ModelPricing {
            input_per_mtok: 1.75,
            output_per_mtok: 14.0,
            cached_input_per_mtok: Some(0.175),
        },
    ),
    (
        "openai-codex",
        "gpt-5.2-pro",
        ModelPricing {
            input_per_mtok: 21.0,
            output_per_mtok: 168.0,
            cached_input_per_mtok: None,
        },
    ),
    (
        "openai-codex",
        "gpt-5.1",
        ModelPricing {
            input_per_mtok: 1.25,
            output_per_mtok: 10.0,
            cached_input_per_mtok: Some(0.125),
        },
    ),
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
        "gpt-5-nano",
        ModelPricing {
            input_per_mtok: 0.05,
            output_per_mtok: 0.40,
            cached_input_per_mtok: Some(0.005),
        },
    ),
    (
        "openai-codex",
        "gpt-5-pro",
        ModelPricing {
            input_per_mtok: 15.0,
            output_per_mtok: 120.0,
            cached_input_per_mtok: None,
        },
    ),
    // The ChatGPT-tuned alias listed under "Specialized models".
    (
        "openai-codex",
        "chat-latest",
        ModelPricing {
            input_per_mtok: 5.0,
            output_per_mtok: 30.0,
            cached_input_per_mtok: Some(0.50),
        },
    ),
    (
        "openai-codex",
        "gpt-4.1",
        ModelPricing {
            input_per_mtok: 2.0,
            output_per_mtok: 8.0,
            cached_input_per_mtok: Some(0.50),
        },
    ),
    (
        "openai-codex",
        "gpt-4.1-mini",
        ModelPricing {
            input_per_mtok: 0.40,
            output_per_mtok: 1.60,
            cached_input_per_mtok: Some(0.10),
        },
    ),
    (
        "openai-codex",
        "gpt-4.1-nano",
        ModelPricing {
            input_per_mtok: 0.10,
            output_per_mtok: 0.40,
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
    // The original gpt-4o snapshot is priced differently from the bare id.
    // `lookup` tries the exact model string before date-stripping, so this
    // entry wins for that snapshot and every other dated gpt-4o falls back
    // to the `gpt-4o` row above.
    (
        "openai-codex",
        "gpt-4o-2024-05-13",
        ModelPricing {
            input_per_mtok: 5.0,
            output_per_mtok: 15.0,
            cached_input_per_mtok: None,
        },
    ),
    (
        "openai-codex",
        "gpt-4o-mini",
        ModelPricing {
            input_per_mtok: 0.15,
            output_per_mtok: 0.60,
            cached_input_per_mtok: Some(0.075),
        },
    ),
    (
        "openai-codex",
        "o1",
        ModelPricing {
            input_per_mtok: 15.0,
            output_per_mtok: 60.0,
            cached_input_per_mtok: Some(7.50),
        },
    ),
    (
        "openai-codex",
        "o1-pro",
        ModelPricing {
            input_per_mtok: 150.0,
            output_per_mtok: 600.0,
            cached_input_per_mtok: None,
        },
    ),
    (
        "openai-codex",
        "o3",
        ModelPricing {
            input_per_mtok: 2.0,
            output_per_mtok: 8.0,
            cached_input_per_mtok: Some(0.50),
        },
    ),
    (
        "openai-codex",
        "o3-pro",
        ModelPricing {
            input_per_mtok: 20.0,
            output_per_mtok: 80.0,
            cached_input_per_mtok: None,
        },
    ),
    (
        "openai-codex",
        "o3-mini",
        ModelPricing {
            input_per_mtok: 1.10,
            output_per_mtok: 4.40,
            cached_input_per_mtok: Some(0.55),
        },
    ),
    (
        "openai-codex",
        "o4-mini",
        ModelPricing {
            input_per_mtok: 1.10,
            output_per_mtok: 4.40,
            cached_input_per_mtok: Some(0.275),
        },
    ),
    // ── Google Gemini ─────────────────────────────────────────────
    // Paid-tier standard rates from
    // https://ai.google.dev/gemini-api/docs/pricing, text modality,
    // prompts <= 200k tokens. Pro models double their rates above 200k
    // input tokens; transcripts don't record the prompt-size tier, so
    // that surcharge is not modeled (same class as OpenAI's long-context
    // tier). Output rates already include thinking tokens, matching
    // rupu's billable-output fold. Batch/Flex/Priority tiers and the
    // per-hour cache storage charge are not modeled. The
    // `google-antigravity` provider serves these same models and
    // resolves against this section.
    // Gemini 3.6/3.7/3.8 Flash carry an introductory rate through
    // 2026-12-31; from 2027-01-01 they double to 1.50 / 7.50 / 0.15.
    (
        "google-gemini-cli",
        "gemini-3.8-flash",
        ModelPricing {
            input_per_mtok: 0.75,
            output_per_mtok: 3.75,
            cached_input_per_mtok: Some(0.075),
        },
    ),
    (
        "google-gemini-cli",
        "gemini-3.7-flash",
        ModelPricing {
            input_per_mtok: 0.75,
            output_per_mtok: 3.75,
            cached_input_per_mtok: Some(0.075),
        },
    ),
    (
        "google-gemini-cli",
        "gemini-3.6-flash",
        ModelPricing {
            input_per_mtok: 0.75,
            output_per_mtok: 3.75,
            cached_input_per_mtok: Some(0.075),
        },
    ),
    (
        "google-gemini-cli",
        "gemini-3.5-flash",
        ModelPricing {
            input_per_mtok: 1.50,
            output_per_mtok: 9.0,
            cached_input_per_mtok: Some(0.15),
        },
    ),
    (
        "google-gemini-cli",
        "gemini-3.5-flash-lite",
        ModelPricing {
            input_per_mtok: 0.30,
            output_per_mtok: 2.50,
            cached_input_per_mtok: Some(0.03),
        },
    ),
    (
        "google-gemini-cli",
        "gemini-3.1-pro-preview",
        ModelPricing {
            input_per_mtok: 2.0,
            output_per_mtok: 12.0,
            cached_input_per_mtok: Some(0.20),
        },
    ),
    (
        "google-gemini-cli",
        "gemini-3.1-pro-preview-customtools",
        ModelPricing {
            input_per_mtok: 2.0,
            output_per_mtok: 12.0,
            cached_input_per_mtok: Some(0.20),
        },
    ),
    (
        "google-gemini-cli",
        "gemini-3.1-flash-lite",
        ModelPricing {
            input_per_mtok: 0.25,
            output_per_mtok: 1.50,
            cached_input_per_mtok: Some(0.025),
        },
    ),
    (
        "google-gemini-cli",
        "gemini-3-flash-preview",
        ModelPricing {
            input_per_mtok: 0.50,
            output_per_mtok: 3.0,
            cached_input_per_mtok: Some(0.05),
        },
    ),
    (
        "google-gemini-cli",
        "gemini-2.5-pro",
        ModelPricing {
            input_per_mtok: 1.25,
            output_per_mtok: 10.0,
            cached_input_per_mtok: Some(0.125),
        },
    ),
    (
        "google-gemini-cli",
        "gemini-2.5-flash",
        ModelPricing {
            input_per_mtok: 0.30,
            output_per_mtok: 2.50,
            cached_input_per_mtok: Some(0.03),
        },
    ),
    (
        "google-gemini-cli",
        "gemini-2.5-flash-lite",
        ModelPricing {
            input_per_mtok: 0.10,
            output_per_mtok: 0.40,
            cached_input_per_mtok: Some(0.01),
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

/// Strip a trailing date snapshot from a model id. Two vendor shapes:
/// OpenAI's `-YYYY-MM-DD` (`gpt-5-2025-08-07`) and Anthropic's compact
/// `-YYYYMMDD` (`claude-opus-4-1-20250805`). Users configure the bare
/// name in agents and the built-in price table keys on it. Returns the
/// original string when no date suffix matches.
fn strip_date_suffix(model: &str) -> &str {
    let bytes = model.as_bytes();
    // `-YYYY-MM-DD`: `-` + 4 digits + `-` + 2 digits + `-` + 2 digits.
    if bytes.len() >= 11 {
        let tail = &bytes[bytes.len() - 11..];
        let dashed = tail[0] == b'-'
            && tail[1..5].iter().all(|c| c.is_ascii_digit())
            && tail[5] == b'-'
            && tail[6..8].iter().all(|c| c.is_ascii_digit())
            && tail[8] == b'-'
            && tail[9..11].iter().all(|c| c.is_ascii_digit());
        if dashed {
            return &model[..model.len() - 11];
        }
    }
    // `-YYYYMMDD`: `-` + exactly 8 digits (the byte before the `-` must
    // not be a digit-run continuation — `-` + 9 digits is not a date).
    if bytes.len() >= 9 {
        let tail = &bytes[bytes.len() - 9..];
        let compact = tail[0] == b'-' && tail[1..].iter().all(|c| c.is_ascii_digit());
        if compact {
            return &model[..model.len() - 9];
        }
    }
    model
}

/// Resolve a USD price for one `(provider, model, agent)` triple.
///
/// `provider` is whatever the run recorded: a vendor key (`anthropic`),
/// a friendly alias (`openai`), or a named account from
/// `[providers.<name>]` (`openai-oracle`). Accounts resolve to their
/// vendor through `cfg.provider_kinds`, and aliases to the canonical
/// `ProviderId` key through [`canonicalize_provider`].
///
/// Lookup order — first non-`None` wins:
///
/// 1. `cfg.models[<provider>][model]` (user-configured), trying the
///    provider as written, then its canonical form, then the account's
///    kind and the kind's canonical form — so a price keyed on the
///    account wins over one keyed on the vendor. Per provider, the model
///    is tried exact, then with a `[<suffix>]` tag stripped
///    (`claude-sonnet-4-6[1m]`), then with a date snapshot stripped.
/// 2. [`BUILTIN_PRICES`] for the canonical vendor key and the same model
///    candidates.
/// 3. `cfg.agents[agent]` (user-configured agent-level fallback)
///
/// Returns `None` when no tier yields a price; callers render that as
/// a placeholder (`—`) in the cost column.
pub fn lookup(
    cfg: &PricingConfig,
    provider: &str,
    model: &str,
    agent: &str,
) -> Option<ModelPricing> {
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

    // Provider keys to try, most specific first: as recorded, its
    // canonical form, then (for a named account) the account's kind
    // and the kind's canonical form. Deduped in order so a plain vendor
    // key is queried once.
    let kind = cfg.provider_kinds.get(provider).map(String::as_str);
    let mut providers: Vec<&str> = Vec::with_capacity(4);
    for prov in [
        Some(provider),
        Some(canonicalize_provider(provider)),
        kind,
        kind.map(canonicalize_provider),
    ]
    .into_iter()
    .flatten()
    {
        if !providers.contains(&prov) {
            providers.push(prov);
        }
    }
    // The built-in table is keyed on the canonical vendor: the kind's
    // canonical form when the provider is an account, else the
    // provider's own.
    let canon_vendor = canonicalize_provider(kind.unwrap_or(provider));

    // Tier 1: user-configured, per provider key, per model candidate.
    for prov in &providers {
        if let Some(per_provider) = cfg.models.get(*prov) {
            for cand in &candidates {
                if let Some(p) = per_provider.get(*cand) {
                    return Some(*p);
                }
            }
        }
    }

    // Tier 2: built-in table — keyed on the canonical vendor.
    for cand in &candidates {
        if let Some(p) = builtin_lookup(canon_vendor, cand) {
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

/// Providers that publish no price list of their own but serve another
/// provider's models at that provider's list rates. Antigravity is the
/// Gemini models behind a different endpoint/quota.
fn builtin_table_provider(canon_provider: &str) -> &str {
    match canon_provider {
        "google-antigravity" => "google-gemini-cli",
        other => other,
    }
}

fn builtin_lookup(provider: &str, model: &str) -> Option<ModelPricing> {
    let table_provider = builtin_table_provider(provider);
    BUILTIN_PRICES
        .iter()
        .find(|(p, m, _)| *p == table_provider && *m == model)
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
    fn openai_current_generation_models_priced_from_builtin() {
        // Standard-tier short-context rates as published on
        // developers.openai.com/api/docs/pricing (reviewed 2026-09-04).
        let cfg = empty_cfg();
        let p = lookup(&cfg, "openai", "gpt-5.4", "any").unwrap();
        assert_eq!(p.input_per_mtok, 2.50);
        assert_eq!(p.output_per_mtok, 15.0);
        assert_eq!(p.cached_input_per_mtok, Some(0.25));

        let cyber = lookup(&cfg, "openai-codex", "gpt-5.6-cyber", "any").unwrap();
        assert_eq!(cyber.input_per_mtok, 12.50);
        assert_eq!(cyber.output_per_mtok, 75.0);
        assert_eq!(cyber.cached_input_per_mtok, Some(1.25));

        let o3 = lookup(&cfg, "openai", "o3", "any").unwrap();
        assert_eq!(o3.input_per_mtok, 2.0);
        assert_eq!(o3.output_per_mtok, 8.0);
        assert_eq!(o3.cached_input_per_mtok, Some(0.50));

        let mini = lookup(&cfg, "openai", "gpt-4.1-mini", "any").unwrap();
        assert_eq!(mini.input_per_mtok, 0.40);
        assert_eq!(mini.output_per_mtok, 1.60);
        assert_eq!(mini.cached_input_per_mtok, Some(0.10));
    }

    #[test]
    fn openai_pro_models_have_no_cached_input_discount() {
        // The pricing page shows no cached-input rate for the pro tier,
        // so cache hits bill at the full input rate rather than a
        // made-up discount.
        let cfg = empty_cfg();
        for model in [
            "gpt-5.5-pro",
            "gpt-5.4-pro",
            "gpt-5.2-pro",
            "gpt-5-pro",
            "o1-pro",
            "o3-pro",
        ] {
            let p = lookup(&cfg, "openai", model, "any").unwrap();
            assert_eq!(p.cached_input_per_mtok, None, "{model}");
            assert!(p.input_per_mtok > 0.0, "{model}");
        }
        let pro = lookup(&cfg, "openai", "gpt-5.5-pro", "any").unwrap();
        assert_eq!(pro.input_per_mtok, 30.0);
        assert_eq!(pro.output_per_mtok, 180.0);
    }

    #[test]
    fn dated_gpt4o_snapshot_keeps_its_own_price_over_date_strip() {
        // OpenAI prices `gpt-4o-2024-05-13` differently from the bare
        // `gpt-4o`. The exact dated entry must win; a snapshot the table
        // doesn't know (`gpt-4o-2024-08-06`) still falls back to `gpt-4o`.
        let cfg = empty_cfg();
        let snapshot = lookup(&cfg, "openai", "gpt-4o-2024-05-13", "any").unwrap();
        assert_eq!(snapshot.input_per_mtok, 5.0);
        assert_eq!(snapshot.output_per_mtok, 15.0);
        assert_eq!(snapshot.cached_input_per_mtok, None);

        let other = lookup(&cfg, "openai", "gpt-4o-2024-08-06", "any").unwrap();
        assert_eq!(other.input_per_mtok, 2.50);
    }

    #[test]
    fn daybreak_aliases_track_their_current_targets() {
        // `gpt-daybreak-blue-latest` → gpt-5.6-sol,
        // `gpt-daybreak-red-latest` → gpt-5.6-cyber (per the pricing page).
        let cfg = empty_cfg();
        let blue = lookup(&cfg, "openai", "gpt-daybreak-blue-latest", "any").unwrap();
        let sol = lookup(&cfg, "openai", "gpt-5.6-sol", "any").unwrap();
        assert_eq!(blue, sol);

        let red = lookup(&cfg, "openai", "gpt-daybreak-red-latest", "any").unwrap();
        let cyber = lookup(&cfg, "openai", "gpt-5.6-cyber", "any").unwrap();
        assert_eq!(red, cyber);
    }

    #[test]
    fn builtin_table_has_no_duplicate_keys_and_only_sane_rates() {
        let mut seen = std::collections::BTreeSet::new();
        for (provider, model, price) in BUILTIN_PRICES {
            assert!(
                seen.insert((*provider, *model)),
                "duplicate builtin price entry for {provider}/{model}"
            );
            for (label, rate) in [
                ("input", Some(price.input_per_mtok)),
                ("output", Some(price.output_per_mtok)),
                ("cached", price.cached_input_per_mtok),
            ] {
                if let Some(rate) = rate {
                    assert!(
                        rate.is_finite() && rate >= 0.0,
                        "{provider}/{model} {label} rate {rate} is not a sane price"
                    );
                }
            }
            if let Some(cached) = price.cached_input_per_mtok {
                assert!(
                    cached <= price.input_per_mtok,
                    "{provider}/{model} cached rate {cached} exceeds input rate {}",
                    price.input_per_mtok
                );
            }
        }
    }

    #[test]
    fn anthropic_current_generation_priced_from_builtin() {
        // Per platform.claude.com/docs/en/about-claude/pricing (reviewed
        // 2026-09-04). Opus 4.7 and Haiku 4.5 were previously carried at
        // the wrong tier's rate.
        let cfg = empty_cfg();
        let opus47 = lookup(&cfg, "anthropic", "claude-opus-4-7", "any").unwrap();
        assert_eq!(opus47.input_per_mtok, 5.0);
        assert_eq!(opus47.output_per_mtok, 25.0);
        assert_eq!(opus47.cached_input_per_mtok, Some(0.50));

        let haiku = lookup(&cfg, "anthropic", "claude-haiku-4-5", "any").unwrap();
        assert_eq!(haiku.input_per_mtok, 1.0);
        assert_eq!(haiku.output_per_mtok, 5.0);
        assert_eq!(haiku.cached_input_per_mtok, Some(0.10));

        let fable = lookup(&cfg, "anthropic", "claude-fable-5-1", "any").unwrap();
        assert_eq!(fable.input_per_mtok, 10.0);
        assert_eq!(fable.output_per_mtok, 50.0);
        assert_eq!(fable.cached_input_per_mtok, Some(0.25));

        let mythos5 = lookup(&cfg, "anthropic", "claude-mythos-5", "any").unwrap();
        assert_eq!(mythos5.input_per_mtok, 10.0);
        assert_eq!(mythos5.cached_input_per_mtok, Some(1.0));

        let sonnet5 = lookup(&cfg, "anthropic", "claude-sonnet-5", "any").unwrap();
        assert_eq!(sonnet5.input_per_mtok, 2.0);
        assert_eq!(sonnet5.output_per_mtok, 10.0);

        let opus41 = lookup(&cfg, "anthropic", "claude-opus-4-1", "any").unwrap();
        assert_eq!(opus41.input_per_mtok, 15.0);
        assert_eq!(opus41.output_per_mtok, 75.0);
    }

    #[test]
    fn anthropic_compact_date_snapshot_resolves_to_base_price() {
        // Anthropic snapshots use `-YYYYMMDD` (no dashes), unlike OpenAI's
        // `-YYYY-MM-DD`. Both must strip.
        let cfg = empty_cfg();
        let p = lookup(&cfg, "anthropic", "claude-opus-4-1-20250805", "any").unwrap();
        assert_eq!(p.input_per_mtok, 15.0);

        let h = lookup(&cfg, "anthropic", "claude-3-5-haiku-20241022", "any").unwrap();
        assert_eq!(h.input_per_mtok, 0.80);
        assert_eq!(h.output_per_mtok, 4.0);

        let s = lookup(&cfg, "anthropic", "claude-haiku-4-5-20251001", "any").unwrap();
        assert_eq!(s.input_per_mtok, 1.0);
    }

    #[test]
    fn strip_date_suffix_strips_compact_yyyymmdd() {
        assert_eq!(
            strip_date_suffix("claude-opus-4-1-20250805"),
            "claude-opus-4-1"
        );
        assert_eq!(
            strip_date_suffix("claude-3-5-sonnet-20241022"),
            "claude-3-5-sonnet"
        );
        // Seven or nine digits are not a date; leave them alone.
        assert_eq!(strip_date_suffix("foo-2025080"), "foo-2025080");
        assert_eq!(strip_date_suffix("foo-202508051"), "foo-202508051");
        // A trailing `-12-2025` (Gemini preview naming) is not YYYYMMDD.
        assert_eq!(
            strip_date_suffix("gemini-2.5-flash-native-audio-preview-12-2025"),
            "gemini-2.5-flash-native-audio-preview-12-2025"
        );
    }

    #[test]
    fn gemini_refreshed_rates_from_builtin() {
        // Per ai.google.dev/gemini-api/docs/pricing (reviewed 2026-09-04),
        // paid tier, <=200k-token prompts, text modality.
        let cfg = empty_cfg();
        let pro = lookup(&cfg, "gemini", "gemini-2.5-pro", "any").unwrap();
        assert_eq!(pro.input_per_mtok, 1.25);
        assert_eq!(pro.output_per_mtok, 10.0);
        assert_eq!(pro.cached_input_per_mtok, Some(0.125));

        let flash = lookup(&cfg, "gemini", "gemini-2.5-flash", "any").unwrap();
        assert_eq!(flash.cached_input_per_mtok, Some(0.03));

        let f38 = lookup(&cfg, "google-gemini-cli", "gemini-3.8-flash", "any").unwrap();
        assert_eq!(f38.input_per_mtok, 0.75);
        assert_eq!(f38.output_per_mtok, 3.75);
        assert_eq!(f38.cached_input_per_mtok, Some(0.075));

        let pro31 = lookup(&cfg, "gemini", "gemini-3.1-pro-preview", "any").unwrap();
        assert_eq!(pro31.input_per_mtok, 2.0);
        assert_eq!(pro31.output_per_mtok, 12.0);
        assert_eq!(pro31.cached_input_per_mtok, Some(0.20));

        let lite = lookup(&cfg, "gemini", "gemini-2.5-flash-lite", "any").unwrap();
        assert_eq!(lite.input_per_mtok, 0.10);
        assert_eq!(lite.output_per_mtok, 0.40);
    }

    #[test]
    fn antigravity_provider_resolves_gemini_builtin_prices() {
        // Antigravity serves the same Gemini models through a different
        // endpoint/quota; without a table of its own it should fall back to
        // the Gemini list rates rather than render unpriced.
        let cfg = empty_cfg();
        let via_antigravity = lookup(&cfg, "antigravity", "gemini-2.5-pro", "any").unwrap();
        let via_canonical = lookup(&cfg, "google-antigravity", "gemini-2.5-pro", "any").unwrap();
        let via_gemini = lookup(&cfg, "gemini", "gemini-2.5-pro", "any").unwrap();
        assert_eq!(via_antigravity, via_gemini);
        assert_eq!(via_canonical, via_gemini);
    }

    #[test]
    fn user_antigravity_entry_wins_over_gemini_fallback() {
        // A user who prices antigravity explicitly must not be overridden
        // by the Gemini fallback.
        let mut cfg = empty_cfg();
        let mut ag = BTreeMap::new();
        ag.insert(
            "gemini-2.5-pro".into(),
            ModelPricing {
                input_per_mtok: 0.0,
                output_per_mtok: 0.0,
                cached_input_per_mtok: None,
            },
        );
        cfg.models.insert("google-antigravity".into(), ag);
        let p = lookup(&cfg, "antigravity", "gemini-2.5-pro", "any").unwrap();
        assert_eq!(p.input_per_mtok, 0.0);
    }

    #[test]
    fn named_provider_account_resolves_builtin_via_its_kind() {
        // `[providers.openai-oracle] kind = "openai"`: runs record the
        // ACCOUNT name as the provider, which is not a vendor key. The
        // lookup must follow the account to its kind and then to the
        // canonical table key (`openai` → `openai-codex`).
        let mut cfg = empty_cfg();
        cfg.provider_kinds
            .insert("openai-oracle".into(), "openai".into());
        let p = lookup(&cfg, "openai-oracle", "gpt-5.6-cyber", "any").unwrap();
        assert_eq!(p.input_per_mtok, 12.50);
        assert_eq!(p.output_per_mtok, 75.0);

        cfg.provider_kinds
            .insert("anthropic-oracle".into(), "anthropic".into());
        let a = lookup(&cfg, "anthropic-oracle", "claude-sonnet-4-6", "any").unwrap();
        assert_eq!(a.input_per_mtok, 3.0);
    }

    #[test]
    fn user_price_keyed_on_account_name_wins_over_kind() {
        // An explicit `[pricing.openai-oracle."gpt-5.6-cyber"]` (say, a
        // negotiated rate on that account) beats the kind's list price.
        let mut cfg = empty_cfg();
        cfg.provider_kinds
            .insert("openai-oracle".into(), "openai".into());
        let mut acct = BTreeMap::new();
        acct.insert(
            "gpt-5.6-cyber".into(),
            ModelPricing {
                input_per_mtok: 1.0,
                output_per_mtok: 2.0,
                cached_input_per_mtok: None,
            },
        );
        cfg.models.insert("openai-oracle".into(), acct);
        let p = lookup(&cfg, "openai-oracle", "gpt-5.6-cyber", "any").unwrap();
        assert_eq!(p.input_per_mtok, 1.0);
    }

    #[test]
    fn user_price_keyed_on_kind_applies_to_every_account_of_that_kind() {
        // `[pricing.openai."gpt-5.6-cyber"]` should cover `openai-oracle`
        // too — the user priced the vendor, not one account.
        let mut cfg = empty_cfg();
        cfg.provider_kinds
            .insert("openai-oracle".into(), "openai".into());
        let mut kind = BTreeMap::new();
        kind.insert(
            "gpt-5.6-cyber".into(),
            ModelPricing {
                input_per_mtok: 7.0,
                output_per_mtok: 8.0,
                cached_input_per_mtok: None,
            },
        );
        cfg.models.insert("openai".into(), kind);
        let p = lookup(&cfg, "openai-oracle", "gpt-5.6-cyber", "any").unwrap();
        assert_eq!(p.input_per_mtok, 7.0);
    }

    #[test]
    fn unknown_account_with_no_kind_still_falls_to_agent_rung() {
        let mut cfg = empty_cfg();
        cfg.agents.insert(
            "reviewer".into(),
            ModelPricing {
                input_per_mtok: 5.0,
                output_per_mtok: 25.0,
                cached_input_per_mtok: None,
            },
        );
        let p = lookup(&cfg, "some-account", "gpt-5.6-cyber", "reviewer").unwrap();
        assert_eq!(p.input_per_mtok, 5.0);
    }

    #[test]
    fn strip_date_suffix_strips_trailing_iso_date() {
        assert_eq!(strip_date_suffix("gpt-5-2025-08-07"), "gpt-5");
        assert_eq!(strip_date_suffix("gpt-4o-mini-2024-07-18"), "gpt-4o-mini");
    }
}

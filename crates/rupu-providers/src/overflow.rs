//! Context-overflow error formats (spec 2026-09-30 §7), shared by the error classifier and the agent runner.

use crate::error::ProviderError;
use crate::reply_error::ErrorClass;

/// A provider context-overflow error, with the numbers when the message
/// carries them (spec 2026-09-30 §7; formats are observed, not documented).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Overflow {
    pub tokens: Option<u32>,
    pub max: Option<u32>,
}

pub fn parse_context_overflow(err: &str) -> Option<Overflow> {
    let e = err.to_ascii_lowercase();
    let after = |needle: &str| e.find(needle).map(|i| &e[i + needle.len()..]);
    // Anthropic: "prompt is too long: N tokens > M maximum"
    if let Some(rest) = after("prompt is too long:") {
        let n = numbers(rest);
        return Some(Overflow {
            tokens: n.first().copied(),
            max: n.get(1).copied(),
        });
    }
    // Anthropic (pre-4.5 validation): "input length and `max_tokens` exceed
    // context limit: A + B > C". The input alone (A) fits; it is the output
    // reservation that does not, so C is no input limit — no max. The runner
    // first lowers the request's cap (`parse_output_cap_overflow`); this arm
    // is the fall-through when that is not possible.
    if let Some(cap) = parse_output_cap_overflow(err) {
        return Some(Overflow {
            tokens: Some(cap.input),
            max: None,
        });
    }
    // GitHub Copilot (CAPI `model_max_prompt_tokens_exceeded`): "prompt token
    // count of N exceeds the limit of M" (microsoft/vscode
    // extensions/copilot/test/inline/inlineEditCode.stest.ts).
    if let Some(rest) = after("prompt token count of") {
        if rest.contains("exceeds the limit of") {
            let n = numbers(rest);
            return Some(Overflow {
                tokens: n.first().copied(),
                max: n.get(1).copied(),
            });
        }
    }
    // OpenAI / Copilot: "maximum context length is M tokens … resulted in N tokens"
    if let Some(rest) = after("maximum context length is") {
        let n = numbers(rest);
        return Some(Overflow {
            max: n.first().copied(),
            tokens: n.get(1).copied(),
        });
    }
    // vLLM: "Input length (N) exceeds model's maximum context length (M)"
    if e.contains("exceeds model's maximum context length") {
        if let Some(rest) = after("input length") {
            let n = numbers(rest);
            return Some(Overflow {
                tokens: n.first().copied(),
                max: n.get(1).copied(),
            });
        }
    }
    // Gemini: "input token count (N) exceeds the maximum number of tokens allowed (M)"
    if let Some(rest) = after("input token count") {
        if rest.contains("exceeds") {
            let n = numbers(rest);
            return Some(Overflow {
                tokens: n.first().copied(),
                max: n.get(1).copied(),
            });
        }
    }
    if e.contains("prompt is too long")
        || e.contains("too many tokens")
        || e.contains("context window")
    {
        return Some(Overflow {
            tokens: None,
            max: None,
        });
    }
    None
}

/// Anthropic's pre-4.5 validation error, "input length and `max_tokens`
/// exceed context limit: A + B > C": the input (A) fits the window (C), the
/// output reservation (B) does not. Wording verified against real API
/// responses quoted in anthropics/claude-code#42 and #228.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputCapOverflow {
    pub input: u32,
    pub max_tokens: u32,
    pub window: u32,
}

/// Room left between the input and the window when lowering the output cap
/// after an [`OutputCapOverflow`] (token counts are estimates on both sides).
const OUTPUT_CAP_MARGIN: u32 = 1000;
/// The smallest output cap worth retrying with; below it, compact instead.
const MIN_LOWERED_OUTPUT_CAP: u32 = 1024;

impl OutputCapOverflow {
    /// `window − input − 1000`, or `None` when that is under 1024 (the input
    /// itself is what has to shrink).
    pub fn lowered_max_tokens(&self) -> Option<u32> {
        self.window
            .checked_sub(self.input)?
            .checked_sub(OUTPUT_CAP_MARGIN)
            .filter(|n| *n >= MIN_LOWERED_OUTPUT_CAP)
    }
}

pub fn parse_output_cap_overflow(err: &str) -> Option<OutputCapOverflow> {
    let e = err.to_ascii_lowercase();
    let needle = "input length and `max_tokens` exceed context limit";
    let rest = &e[e.find(needle)? + needle.len()..];
    let n = numbers(rest);
    Some(OutputCapOverflow {
        input: *n.first()?,
        max_tokens: *n.get(1)?,
        window: *n.get(2)?,
    })
}

/// Every integer in `s`, in order. A `,` or `_` between digits is a thousands separator.
fn numbers(s: &str) -> Vec<u32> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut cur: Option<u64> = None;
    for (i, &c) in b.iter().enumerate() {
        if c.is_ascii_digit() {
            cur = Some(
                cur.unwrap_or(0)
                    .saturating_mul(10)
                    .saturating_add(u64::from(c - b'0')),
            );
        } else if (c == b',' || c == b'_')
            && cur.is_some()
            && b.get(i + 1).is_some_and(u8::is_ascii_digit)
        {
            // thousands separator
        } else if let Some(n) = cur.take() {
            out.push(n.min(u64::from(u32::MAX)) as u32);
        }
    }
    if let Some(n) = cur {
        out.push(n.min(u64::from(u32::MAX)) as u32);
    }
    out
}

/// Overflow read off an error (spec 2026-10-01 §4.4): a reply body's
/// message is parsed for numbers; a `ContextOverflow`-class reply is an
/// overflow even when no numbers parse. Errors without a body fall back to
/// their display text (the three generic phrases still match there).
pub fn context_overflow_of(e: &ProviderError) -> Option<Overflow> {
    match e {
        ProviderError::Reply(b) => parse_context_overflow(&b.message).or((b.class
            == ErrorClass::ContextOverflow)
            .then_some(Overflow {
                tokens: None,
                max: None,
            })),
        other => parse_context_overflow(&other.to_string()),
    }
}

pub fn output_cap_overflow_of(e: &ProviderError) -> Option<OutputCapOverflow> {
    match e {
        ProviderError::Reply(b) => parse_output_cap_overflow(&b.message),
        other => parse_output_cap_overflow(&other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overflow_formats_parse_tokens_and_max() {
        let cases = [
            (
                r#"bad request: {"type":"error","error":{"type":"invalid_request_error","message":"prompt is too long: 215000 tokens > 200000 maximum"}}"#,
                Some(215_000),
                Some(200_000),
            ),
            (
                "API error 400: This model's maximum context length is 128000 tokens. However, your messages resulted in 130500 tokens.",
                Some(130_500),
                Some(128_000),
            ),
            (
                "API error 400: Input length (140000) exceeds model's maximum context length (131072).",
                Some(140_000),
                Some(131_072),
            ),
            (
                "bad request: The input token count (1100000) exceeds the maximum number of tokens allowed (1048576).",
                Some(1_100_000),
                Some(1_048_576),
            ),
            ("prompt is too long for the model", None, None),
            ("too many tokens in request", None, None),
            ("exceeds context window limit", None, None),
            ("PROMPT IS TOO LONG", None, None), // case-insensitive
        ];
        for (msg, tokens, max) in cases {
            assert_eq!(
                parse_context_overflow(msg),
                Some(Overflow { tokens, max }),
                "{msg}"
            );
        }
    }

    /// Anthropic's pre-4.5 validation error: `input + max_tokens > window`.
    /// Verbatim wording from real API responses quoted in
    /// anthropics/claude-code#42 and #228 (numbers invented).
    const ANTHROPIC_INPUT_PLUS_MAX_TOKENS: &str = r#"API error 400: {"type":"error","error":{"type":"invalid_request_error","message":"input length and `max_tokens` exceed context limit: 183500 + 20000 > 201000, decrease input length or `max_tokens` and try again"}}"#;

    #[test]
    fn input_plus_max_tokens_format_parses_all_three_numbers() {
        assert_eq!(
            parse_output_cap_overflow(ANTHROPIC_INPUT_PLUS_MAX_TOKENS),
            Some(OutputCapOverflow {
                input: 183_500,
                max_tokens: 20_000,
                window: 201_000,
            })
        );
        // Thousands separators, and case-insensitive.
        assert_eq!(
            parse_output_cap_overflow(
                "INPUT LENGTH AND `MAX_TOKENS` EXCEED CONTEXT LIMIT: 1,000 + 2,000 > 2,500"
            ),
            Some(OutputCapOverflow {
                input: 1_000,
                max_tokens: 2_000,
                window: 2_500,
            })
        );
        // Not this format.
        assert_eq!(
            parse_output_cap_overflow("prompt is too long: 215000 tokens > 200000 maximum"),
            None
        );
        // The format with its numbers missing is not usable for lowering.
        assert_eq!(
            parse_output_cap_overflow("input length and `max_tokens` exceed context limit"),
            None
        );
    }

    /// `input + max_tokens > window` lowers the output cap to
    /// `window − input − 1000`, floor 1024; below the floor there is nothing
    /// to lower to.
    #[test]
    fn input_plus_max_tokens_lowered_cap_keeps_a_margin_and_a_floor() {
        let c = |input, window| OutputCapOverflow {
            input,
            max_tokens: 20_000,
            window,
        };
        assert_eq!(c(183_500, 201_000).lowered_max_tokens(), Some(16_500));
        assert_eq!(
            c(100, 2_124).lowered_max_tokens(),
            Some(1_024),
            "exactly the floor"
        );
        assert_eq!(c(100, 2_123).lowered_max_tokens(), None, "below the floor");
        assert_eq!(
            c(205_000, 201_000).lowered_max_tokens(),
            None,
            "input alone overflows"
        );
    }

    /// The same error, when the cap cannot be lowered, is still an overflow:
    /// the input count is known, the input LIMIT is not (the window `C`
    /// counts output too), so no max.
    #[test]
    fn input_plus_max_tokens_is_an_overflow_without_an_input_max() {
        assert_eq!(
            parse_context_overflow(ANTHROPIC_INPUT_PLUS_MAX_TOKENS),
            Some(Overflow {
                tokens: Some(183_500),
                max: None
            })
        );
    }

    /// GitHub Copilot (CAPI) `model_max_prompt_tokens_exceeded`. Verbatim
    /// wording from microsoft/vscode `extensions/copilot/test/inline/
    /// inlineEditCode.stest.ts` (the vscode-copilot-chat sources).
    #[test]
    fn copilot_prompt_limit_format_parses_tokens_and_max() {
        assert_eq!(
            parse_context_overflow(
                r#"API error 400: {"error":{"message":"prompt token count of 13613 exceeds the limit of 12288","code":"model_max_prompt_tokens_exceeded"}}"#
            ),
            Some(Overflow {
                tokens: Some(13_613),
                max: Some(12_288)
            })
        );
    }

    /// Anthropic's extra-usage 429 is triggered by the 1M beta header, not by
    /// the request's size (see the `anthropic-beta` comment in
    /// rupu-providers/src/anthropic.rs), so it is no overflow: compacting
    /// cannot make the same request acceptable. The client turns it into
    /// `ProviderError::LongContextUnavailable`, handled on its own.
    #[test]
    fn long_context_entitlement_429_is_not_an_overflow() {
        assert_eq!(
            parse_context_overflow(
                "API error 429: Extra usage is required for long context requests"
            ),
            None
        );
    }

    /// The Gemini arm needs the "exceeds" verb: a message that merely
    /// mentions an input token count is not an overflow.
    #[test]
    fn gemini_arm_requires_exceeds() {
        assert_eq!(
            parse_context_overflow(
                "API error 500: internal error (input token count 1200, output token count 30)"
            ),
            None
        );
    }

    #[test]
    fn overflow_ignores_unrelated_errors() {
        for msg in ["network error", "invalid api key", "rate limited"] {
            assert_eq!(parse_context_overflow(msg), None);
        }
    }

    #[test]
    fn overflow_numbers_accept_thousands_separators() {
        assert_eq!(
            parse_context_overflow("prompt is too long: 215,000 tokens > 200,000 maximum"),
            Some(Overflow {
                tokens: Some(215_000),
                max: Some(200_000)
            })
        );
    }

    #[test]
    fn context_overflow_of_reads_the_reply_class() {
        let e = ProviderError::api(
            "openai-codex",
            400,
            r#"{"error":{"message":"too big","code":"context_length_exceeded"}}"#,
        );
        assert_eq!(
            context_overflow_of(&e),
            Some(Overflow {
                tokens: None,
                max: None
            })
        );
    }

    #[test]
    fn context_overflow_of_parses_numbers_from_the_reply_message() {
        let e = ProviderError::api(
            "anthropic",
            400,
            r#"{"type":"error","error":{"type":"invalid_request_error","message":"prompt is too long: 215000 tokens > 200000 maximum"}}"#,
        );
        assert_eq!(
            context_overflow_of(&e),
            Some(Overflow {
                tokens: Some(215_000),
                max: Some(200_000)
            })
        );
    }

    #[test]
    fn context_overflow_of_falls_back_to_display_text_for_other_errors() {
        let e = ProviderError::Other(anyhow::anyhow!("prompt is too long: 10 tokens > 5 maximum"));
        assert_eq!(
            context_overflow_of(&e),
            Some(Overflow {
                tokens: Some(10),
                max: Some(5)
            })
        );
        assert_eq!(
            context_overflow_of(&ProviderError::Http("boom".into())),
            None
        );
    }
}

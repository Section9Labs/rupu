//! One-row rendering of the response-outcome transcript events, shared by
//! every CLI transcript renderer (`transcript`, `session`, the workflow
//! printer, `autoflow serve`, `watch`, `run`). Text comes from
//! `rupu_transcript::outcome`; this module picks the tone and bounds the
//! width.
//!
//! Every CLI renderer prints `status.glyph()` ahead of the label, so the
//! detail returned here is glyph-free (`✗`/`!`/`↺` would repeat it). The tone
//! is chosen so that glyph reads correctly: `✗` error, `!` warning, `●` info,
//! `↺` recovery. Never `⏸` (Awaiting), which would suggest the run paused.

use rupu_transcript::{Event, Severity};

use super::palette::Status;
use super::printer::LineStreamPrinter;
use crate::cmd::transcript::truncate_single_line;

/// Column cap for an outcome / recovery detail, matching `Notice` rows.
const ROW_DETAIL_MAX: usize = 96;
/// Unknown rows carry the tag plus up to 200 chars of data.
const UNKNOWN_ROW_MAX: usize = 240;

/// Tone for an outcome: failures read as failures, warnings as soft
/// failures, info as a neutral active row.
pub fn outcome_status(severity: Severity) -> Status {
    match severity {
        Severity::Error => Status::Failed,
        Severity::Warning => Status::SoftFailed,
        Severity::Info => Status::Active,
    }
}

/// `(tone, label, detail)` for `Outcome`, `Recovery`, `AssistantBlock` and
/// `Unknown` events; `None` for every other variant. The detail is a single line.
pub fn outcome_event_row(ev: &Event) -> Option<(Status, &'static str, String)> {
    match ev {
        Event::Outcome { outcome, .. } => Some((
            outcome_status(outcome.severity),
            "outcome",
            truncate_single_line(
                &rupu_transcript::outcome::outcome_body(outcome),
                ROW_DETAIL_MAX,
            ),
        )),
        Event::Recovery {
            rung,
            action,
            attempt,
            budget,
            provider,
            model,
            reason,
            ..
        } => Some((
            Status::Retrying,
            "recovery",
            truncate_single_line(
                &rupu_transcript::outcome::recovery_body(
                    *action,
                    *rung,
                    provider.as_deref(),
                    model.as_deref(),
                    *attempt,
                    *budget,
                    reason.as_deref(),
                ),
                ROW_DETAIL_MAX,
            ),
        )),
        Event::Unknown { tag, data } => Some((
            Status::Active,
            "event",
            truncate_single_line(
                &rupu_transcript::outcome::unknown_line(tag, data),
                UNKNOWN_ROW_MAX,
            ),
        )),
        // A reply block with no event of its own: a server-side fallback
        // boundary, an unrecognized block (raw JSON, cut like an unknown
        // event), or a block abandoned at a mid-output fallback. Info rows.
        Event::AssistantBlock { block, abandoned } => Some((
            Status::Active,
            "block",
            truncate_single_line(
                &rupu_transcript::outcome::assistant_block_line(block, *abandoned),
                UNKNOWN_ROW_MAX,
            ),
        )),
        _ => None,
    }
}

/// Print an `Outcome` / `Recovery` / `AssistantBlock` / `Unknown` event as a
/// sideband row on a pretty live printer. Returns `false` for any other
/// variant. Shared by the
/// `run` live printer, `watch` replay, the workflow printer's `process_event`
/// and the pretty transcript renderer, so none of them can drop one of these.
pub fn print_outcome_event(printer: &mut LineStreamPrinter, ev: &Event) -> bool {
    match outcome_event_row(ev) {
        Some((status, label, text)) => {
            printer.sideband_event(status, label, Some(&text));
            true
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rupu_transcript::{OutcomeRecord, RecoveryAction};

    fn outcome(severity: Severity, title: &str, detail: Option<&str>) -> Event {
        Event::Outcome {
            turn_idx: 0,
            outcome: OutcomeRecord {
                id: "oc_1".into(),
                class: "refusal".into(),
                severity,
                title: title.into(),
                detail: detail.map(str::to_string),
                error_class: None,
                wire: serde_json::Value::Null,
            },
        }
    }

    fn recovery(action: RecoveryAction, reason: Option<&str>) -> Event {
        Event::Recovery {
            outcome_id: "oc_1".into(),
            rung: 1,
            action,
            attempt: None,
            budget: None,
            provider: None,
            model: None,
            reason: reason.map(str::to_string),
            merge_into_previous: false,
            continues_output: false,
        }
    }

    #[test]
    fn rows_carry_no_glyph_of_their_own_and_never_the_pause_tone() {
        let (st, label, text) = outcome_event_row(&outcome(
            Severity::Error,
            "refused · cyber",
            Some("Declined for this example."),
        ))
        .unwrap();
        assert_eq!(st, Status::Failed);
        assert_eq!(label, "outcome");
        assert_eq!(text, "refused · cyber — Declined for this example.");

        let (st, _, text) =
            outcome_event_row(&outcome(Severity::Warning, "cut off", None)).unwrap();
        assert_eq!((st, text.as_str()), (Status::SoftFailed, "cut off"));

        let (st, _, text) = outcome_event_row(&outcome(Severity::Info, "paused", None)).unwrap();
        assert_eq!((st, text.as_str()), (Status::Active, "paused"));

        let (st, label, text) =
            outcome_event_row(&recovery(RecoveryAction::Retried, None)).unwrap();
        assert_eq!(st, Status::Retrying);
        assert_eq!(label, "recovery");
        assert_eq!(text, "rung 1 · retried");

        for ev in [
            outcome(Severity::Error, "e", None),
            outcome(Severity::Warning, "w", None),
            outcome(Severity::Info, "i", None),
            recovery(RecoveryAction::Retried, None),
        ] {
            let (st, _, text) = outcome_event_row(&ev).unwrap();
            assert_ne!(st, Status::Awaiting, "no pause glyph on {ev:?}");
            for glyph in ['✗', '!', '↺', '⏸'] {
                assert!(!text.contains(glyph), "{text:?} repeats {glyph}");
            }
        }
    }

    #[test]
    fn a_long_multi_line_detail_becomes_one_bounded_row() {
        let detail = format!("first line\n{}\nlast line", "word ".repeat(200));
        let (_, _, text) =
            outcome_event_row(&outcome(Severity::Error, "refused · cyber", Some(&detail))).unwrap();
        assert!(!text.contains('\n'));
        assert_eq!(text.chars().count(), ROW_DETAIL_MAX);
        assert!(text.ends_with('…'));
        assert!(text.starts_with("refused · cyber — first line word"));

        let reason = format!("no\ncredentials {}", "x".repeat(300));
        let (_, _, text) =
            outcome_event_row(&recovery(RecoveryAction::Skipped, Some(&reason))).unwrap();
        assert!(!text.contains('\n'));
        assert_eq!(text.chars().count(), ROW_DETAIL_MAX);

        let unknown = Event::Unknown {
            tag: "new\nevent".into(),
            data: serde_json::json!({"x": "y".repeat(500)}),
        };
        let (_, _, text) = outcome_event_row(&unknown).unwrap();
        assert!(!text.contains('\n'));
        assert!(text.starts_with("unrecognized event · new event"));
        assert!(text.chars().count() <= UNKNOWN_ROW_MAX);

        let block = Event::AssistantBlock {
            block: serde_json::json!({"type": "unknown", "raw": {"type": "x", "pad": "y".repeat(500)}}),
            abandoned: false,
        };
        let (_, _, text) = outcome_event_row(&block).unwrap();
        assert!(!text.contains('\n'));
        assert!(text.starts_with("unrecognized block · x"));
        assert!(text.chars().count() <= UNKNOWN_ROW_MAX);
    }

    #[test]
    fn sideband_rows_have_exactly_one_glyph_and_never_the_pause_glyph() {
        crate::output::palette::disable_color();
        let printer = LineStreamPrinter::new();
        let unknown = Event::Unknown {
            tag: "brand_new_event".into(),
            data: serde_json::json!({"x": 1}),
        };
        let cases = [
            (
                outcome(Severity::Error, "refused · cyber", Some("Declined.")),
                "│  ✗ outcome  refused · cyber — Declined.",
            ),
            (
                outcome(Severity::Warning, "cut off", None),
                "│  ! outcome  cut off",
            ),
            (
                outcome(Severity::Info, "paused", None),
                "│  ● outcome  paused",
            ),
            (
                recovery(RecoveryAction::Retried, None),
                "│  ↺ recovery  rung 1 · retried",
            ),
            (
                unknown,
                "│  ● event  unrecognized event · brand_new_event {\"x\":1}",
            ),
            (
                Event::AssistantBlock {
                    block: serde_json::json!({
                        "type": "fallback", "from_model": "model-a", "to_model": "model-b"
                    }),
                    abandoned: false,
                },
                "│  ● block  served by fallback · model-a → model-b",
            ),
            (
                Event::AssistantBlock {
                    block: serde_json::json!({
                        "type": "unknown", "raw": {"type": "lantern_note"}
                    }),
                    abandoned: false,
                },
                "│  ● block  unrecognized block · lantern_note {\"type\":\"lantern_note\"}",
            ),
            (
                Event::AssistantBlock {
                    block: serde_json::json!({
                        "type": "tool_use", "id": "c1", "name": "bash", "input": {}
                    }),
                    abandoned: true,
                },
                "│  ● block  abandoned · tool call · bash",
            ),
        ];
        for (ev, expected) in &cases {
            let (status, label, text) = outcome_event_row(ev).expect("row");
            let row = printer.sideband_event_text(status, label, Some(&text));
            assert_eq!(&row, expected);
            assert!(!row.contains('⏸'), "{row}");
        }
    }

    #[test]
    fn print_outcome_event_covers_all_four_and_only_those() {
        crate::output::palette::disable_color();
        let mut printer = LineStreamPrinter::new();
        let unknown = Event::Unknown {
            tag: "brand_new_event".into(),
            data: serde_json::Value::Null,
        };
        assert!(print_outcome_event(&mut printer, &unknown));
        assert!(print_outcome_event(
            &mut printer,
            &outcome(Severity::Info, "paused", None)
        ));
        assert!(print_outcome_event(
            &mut printer,
            &recovery(RecoveryAction::Retried, None)
        ));
        assert!(print_outcome_event(
            &mut printer,
            &Event::AssistantBlock {
                block: serde_json::json!({"type": "unknown", "raw": null}),
                abandoned: false,
            }
        ));
        assert!(!print_outcome_event(
            &mut printer,
            &Event::UserMessage {
                content: "hi".into()
            }
        ));
    }
}

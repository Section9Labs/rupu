//! One-row rendering of the response-outcome transcript events, shared by
//! every CLI transcript renderer (`transcript`, `session`, the workflow
//! printer, `autoflow serve`, `watch`, `run`). Text comes from
//! `rupu_transcript::outcome`; this module only picks the tone.

use rupu_transcript::{Event, Severity};

use super::palette::Status;

/// Tone for an outcome: failures read as failures, warnings as soft
/// failures, info as a notice.
pub fn outcome_status(severity: Severity) -> Status {
    match severity {
        Severity::Error => Status::Failed,
        Severity::Warning => Status::SoftFailed,
        Severity::Info => Status::Awaiting,
    }
}

/// `(tone, label, detail)` for `Outcome`, `Recovery` and `Unknown` events;
/// `None` for every other variant.
pub fn outcome_event_row(ev: &Event) -> Option<(Status, &'static str, String)> {
    match ev {
        Event::Outcome { outcome, .. } => Some((
            outcome_status(outcome.severity),
            "outcome",
            rupu_transcript::outcome::outcome_line(outcome),
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
            Status::Awaiting,
            "recovery",
            rupu_transcript::outcome::recovery_line(
                *action,
                *rung,
                provider.as_deref(),
                model.as_deref(),
                *attempt,
                *budget,
                reason.as_deref(),
            ),
        )),
        Event::Unknown { tag, data } => Some((
            Status::Active,
            "event",
            rupu_transcript::outcome::unknown_line(tag, data),
        )),
        _ => None,
    }
}

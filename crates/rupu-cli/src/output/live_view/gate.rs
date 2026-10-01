//! The gate-details panel behind `v` / `enter` at a focused approval gate
//! (spec 2026-09-30, "Gate handling"): why the run is parked and what it has
//! found so far. Pure — the caller reads the findings off disk and hands them
//! in, so the panel is unit-testable and never does I/O of its own.

use chrono::{DateTime, Utc};

use crate::output::live_view::layout::printable;
use crate::output::live_view::row::Line;
use crate::output::palette::Status;
use crate::output::run_model::{fmt_hms, GateView};

/// Findings listed before the rest collapse into a `+N more` row.
const MAX_FINDINGS: usize = 8;
/// Prompt lines shown (an approval prompt is usually a sentence or two).
const MAX_PROMPT_LINES: usize = 3;

/// One finding recorded by the run so far (a step result's panel finding).
/// All text is untrusted wire text; [`gate_detail_lines`] scrubs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateFinding {
    pub severity: String,
    pub title: String,
    /// The reporting panelist / agent, when known.
    pub who: Option<String>,
}

/// Higher = more severe; unknown words sort last.
fn severity_rank(severity: &str) -> u8 {
    match severity.trim().to_ascii_lowercase().as_str() {
        "critical" => 5,
        "high" => 4,
        "medium" => 3,
        "low" => 2,
        "info" => 1,
        _ => 0,
    }
}

fn gap_ms(later: DateTime<Utc>, earlier: DateTime<Utc>) -> u64 {
    u64::try_from((later - earlier).num_milliseconds()).unwrap_or(0)
}

/// The panel rows for `gate`: a header (step, how long it has been parked,
/// when it expires), the approval prompt, then the findings behind it, most
/// severe first.
pub fn gate_detail_lines(
    gate: &GateView,
    findings: &[GateFinding],
    now: DateTime<Utc>,
) -> Vec<Line> {
    let mut head = Line::new()
        .status(Status::Awaiting, "⏸ ")
        .strong(printable(&gate.step_id))
        .dim(format!(" · parked {}", fmt_hms(gap_ms(now, gate.since))));
    head = match gate.expires_at {
        Some(exp) if exp > now => head.dim(format!(" · expires in {}", fmt_hms(gap_ms(exp, now)))),
        Some(exp) => head.danger(format!(" · expired {} ago", fmt_hms(gap_ms(now, exp)))),
        None => head,
    };
    let mut rows = vec![head];

    if let Some(prompt) = &gate.prompt {
        rows.extend(
            prompt
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .take(MAX_PROMPT_LINES)
                .map(|l| Line::new().plain(format!("  {}", printable(l)))),
        );
    }

    if findings.is_empty() {
        rows.push(Line::new().dim("⚑ no findings recorded for this run"));
        return rows;
    }
    rows.push(Line::new().meter(format!(
        "⚑ {} finding{} behind this gate",
        findings.len(),
        if findings.len() == 1 { "" } else { "s" }
    )));
    let mut ordered: Vec<&GateFinding> = findings.iter().collect();
    ordered.sort_by_key(|f| std::cmp::Reverse(severity_rank(&f.severity)));
    for f in ordered.iter().take(MAX_FINDINGS) {
        let sev = printable(&f.severity.trim().to_ascii_uppercase());
        let mut line = if severity_rank(&f.severity) >= 4 {
            Line::new().plain("  ").danger(format!("{sev:<8}"))
        } else {
            Line::new().plain("  ").meter(format!("{sev:<8}"))
        };
        line = line.plain(printable(f.title.trim()));
        if let Some(who) = f.who.as_deref().filter(|w| !w.trim().is_empty()) {
            line = line.dim(format!(" · {}", printable(who.trim())));
        }
        rows.push(line);
    }
    let hidden = ordered.len().saturating_sub(MAX_FINDINGS);
    if hidden > 0 {
        rows.push(Line::new().dim(format!("  +{hidden} more")));
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::live_view::row::render_plain;
    use chrono::{Duration, TimeZone};

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 30, 12, 0, 0).unwrap()
    }

    fn gate() -> GateView {
        GateView {
            step_id: "approve".into(),
            prompt: Some("Publish the report?\nIt will be emailed to the team.".into()),
            since: now() - Duration::seconds(252),
            expires_at: Some(now() + Duration::seconds(180)),
        }
    }

    fn finding(sev: &str, title: &str, who: Option<&str>) -> GateFinding {
        GateFinding {
            severity: sev.into(),
            title: title.into(),
            who: who.map(str::to_string),
        }
    }

    #[test]
    fn panel_shows_the_gate_its_prompt_and_findings_most_severe_first() {
        let rows = gate_detail_lines(
            &gate(),
            &[
                finding("low", "Verbose logging", None),
                finding("High", "SQL injection in login", Some("otter#2")),
                finding("medium", "Missing CSRF token", None),
            ],
            now(),
        );
        assert_eq!(
            render_plain(&rows),
            "⏸ approve · parked 4m 12s · expires in 3m 00s\n  Publish the report?\n  \
             It will be emailed to the team.\n⚑ 3 findings behind this gate\n  \
             HIGH    SQL injection in login · otter#2\n  MEDIUM  Missing CSRF token\n  \
             LOW     Verbose logging"
        );
    }

    #[test]
    fn panel_is_honest_when_the_run_has_no_findings() {
        let mut g = gate();
        g.prompt = None;
        g.expires_at = None;
        let s = render_plain(&gate_detail_lines(&g, &[], now()));
        assert_eq!(
            s,
            "⏸ approve · parked 4m 12s\n⚑ no findings recorded for this run"
        );
    }

    #[test]
    fn an_overdue_gate_says_so_and_a_long_list_is_capped() {
        let mut g = gate();
        g.prompt = None;
        g.expires_at = Some(now() - Duration::seconds(65));
        let many: Vec<GateFinding> = (0..10)
            .map(|i| finding("low", &format!("finding {i}"), None))
            .collect();
        let s = render_plain(&gate_detail_lines(&g, &many, now()));
        assert!(s.contains("expired 1m 05s ago"), "{s}");
        assert!(s.contains("⚑ 10 findings behind this gate"), "{s}");
        assert_eq!(s.matches("LOW").count(), MAX_FINDINGS, "{s}");
        assert!(s.ends_with("  +2 more"), "{s}");
    }

    #[test]
    fn untrusted_text_never_reaches_the_terminal_raw() {
        let mut g = gate();
        g.step_id = "ap\u{1b}[2Jprove".into();
        g.prompt = Some("ok\u{1b}]0;pwned\u{7}\n\u{1b}[31mred".into());
        let rows = gate_detail_lines(
            &g,
            &[finding(
                "hi\u{1b}gh",
                "ti\u{1b}[2Jtle\nx",
                Some("wh\u{1b}o"),
            )],
            now(),
        );
        let s = render_plain(&rows);
        assert!(!s.contains('\u{1b}') && !s.contains('\u{7}'), "{s:?}");
        // One row per logical line: nothing smuggled a newline into a row.
        assert_eq!(
            rows.iter()
                .map(|l| render_plain(std::slice::from_ref(l)))
                .filter(|r| r.contains('\n'))
                .count(),
            0
        );
    }
}

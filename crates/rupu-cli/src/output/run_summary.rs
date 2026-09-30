//! Completion-summary formatter: a `RunView` → the block printed when a
//! `workflow run` / `resume` finishes (or parks at a gate). Shared by the
//! live view (A), retained view (B), and line printer (C).

use chrono::{DateTime, Utc};

use crate::output::palette::Status;
use crate::output::run_model::{fmt_hms, step_status, RunView, UnitStatus};
use rupu_orchestrator::runs::RunStatus;

fn compact_run_id(id: &str) -> String {
    // Reuse the house compaction if available; fall back to a head…tail.
    crate::output::ids::compact_id(id)
}

pub fn render_completion_summary(v: &RunView, now: DateTime<Utc>) -> String {
    let mut out = String::new();
    let elapsed = v.elapsed_ms(now).map(fmt_hms).unwrap_or_default();

    if v.status == RunStatus::AwaitingApproval && !v.gates.is_empty() {
        out.push_str(&format!(
            "{} {} · {} awaiting approval · {}\n",
            Status::Awaiting.glyph(),
            v.workflow_name,
            v.crew.as_deref().unwrap_or("-"),
            elapsed,
        ));
        for g in &v.gates {
            out.push_str(&format!(
                "  gate · {} — {}\n",
                g.step_id,
                g.prompt.as_deref().unwrap_or("approve to continue"),
            ));
        }
        out.push_str(&format!(
            "  approve  rupu workflow approve {}\n  reject   rupu workflow reject {}\n",
            compact_run_id(&v.run_id),
            compact_run_id(&v.run_id),
        ));
        return out;
    }

    let status_glyph = match v.status {
        RunStatus::Completed => Status::Complete,
        RunStatus::Failed | RunStatus::Rejected | RunStatus::Cancelled => Status::Failed,
        RunStatus::Paused => Status::Waiting,
        _ => Status::Working,
    }
    .glyph();
    out.push_str(&format!(
        "{} {} · {}    {:?} · {}\n",
        status_glyph,
        v.workflow_name,
        v.crew.as_deref().unwrap_or("-"),
        v.status,
        elapsed,
    ));

    // steps line
    let mut steps = String::from("steps   ");
    for s in &v.steps {
        let g = step_status(s.state).glyph();
        let c = s.unit_counts();
        if c.total > 0 {
            steps.push_str(&format!("{} {} ({}/{})  ", g, s.step_id, c.done, c.total));
        } else {
            steps.push_str(&format!("{} {}  ", g, s.step_id));
        }
    }
    out.push_str(steps.trim_end());
    out.push('\n');

    // units line (only when there are units anywhere)
    let (ok, failed): (usize, usize) =
        v.steps
            .iter()
            .flat_map(|s| s.units.values())
            .fold((0, 0), |(ok, bad), u| match u.status {
                UnitStatus::Done => (ok + 1, bad),
                UnitStatus::Failed => (ok, bad + 1),
                _ => (ok, bad),
            });
    if ok + failed > 0 {
        out.push_str(&format!("units   {ok} ok · {failed} failed\n"));
    }

    // totals line (cost only when priced)
    if let Some(u) = &v.usage {
        let mut line = format!("tokens  ⇡{} ⇣{}", u.input_tokens, u.output_tokens);
        if let Some(cost) = u.cost_usd {
            if u.priced {
                line.push_str(&format!("  ${cost:.2}"));
            }
        }
        out.push_str(&line);
        out.push('\n');
    }

    // findings (only when present)
    if !v.findings_by_severity.is_empty() {
        let total: usize = v.findings_by_severity.values().sum();
        let mut parts = vec![format!("findings {total}")];
        for (sev, n) in &v.findings_by_severity {
            parts.push(format!("{n} {}", sev.to_uppercase()));
        }
        out.push_str(&parts.join(" · "));
        out.push('\n');
    }

    // failed units
    let failed_units: Vec<String> = v
        .steps
        .iter()
        .flat_map(|s| s.units.values())
        .filter(|u| u.status == UnitStatus::Failed)
        .map(|u| match &u.codename {
            Some(c) => format!("{c} ({})", u.unit_key),
            None => u.unit_key.clone(),
        })
        .collect();
    if !failed_units.is_empty() {
        out.push_str(&format!(
            "failed  {}  → resume to retry\n",
            failed_units.join("  ")
        ));
    }

    out.push_str(&format!(
        "view    rupu workflow show-run {}\n",
        compact_run_id(&v.run_id)
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::run_model::*;
    use chrono::{TimeZone, Utc};
    use rupu_orchestrator::runs::{RunStatus, StepKind};

    fn now() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 30, 12, 0, 0).unwrap()
    }

    fn completed_view() -> RunView {
        let mut v = RunView::default();
        v.run_id = "run_01M1SSABCDEFG".into();
        v.workflow_name = "assess-services".into();
        v.crew = Some("mint-tundra".into());
        v.status = RunStatus::Completed;
        v.started_at = Some(Utc.with_ymd_and_hms(2026, 9, 30, 10, 12, 40).unwrap());
        v.finished_at = Some(now());
        v.usage = Some(rupu_cp::usage::UsageSummary {
            input_tokens: 6_100_000,
            output_tokens: 420_000,
            total_tokens: 6_520_000,
            cost_usd: Some(18.40),
            priced: true,
            ..Default::default()
        });
        // one completed linear step + one fan-out with 2 failed
        v.apply(&rupu_orchestrator::executor::Event::StepStarted {
            run_id: "r".into(),
            step_id: "preflight".into(),
            kind: StepKind::Run,
            agent: None,
            host: None,
            codename: None,
        });
        v.apply(&rupu_orchestrator::executor::Event::StepCompleted {
            run_id: "r".into(),
            step_id: "preflight".into(),
            success: true,
            duration_ms: 18000,
            host: None,
        });
        v
    }

    #[test]
    fn completed_summary_snapshot() {
        let s = render_completion_summary(&completed_view(), now());
        insta::assert_snapshot!(s);
    }

    #[test]
    fn cost_omitted_when_unpriced() {
        let mut v = completed_view();
        v.usage = Some(rupu_cp::usage::UsageSummary {
            input_tokens: 100,
            output_tokens: 50,
            total_tokens: 150,
            cost_usd: None,
            priced: false,
            ..Default::default()
        });
        let s = render_completion_summary(&v, now());
        assert!(
            !s.contains('$'),
            "unpriced run must not print a fake cost:\n{s}"
        );
    }
}

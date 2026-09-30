//! Completion-summary formatter: a `RunView` → the block printed when a
//! `workflow run` / `resume` finishes (or parks at a gate). Shared by the
//! live view (A), retained view (B), and line printer (C).

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};

use crate::output::palette::Status;
use crate::output::run_model::{fmt_hms, step_status, RunView, UnitStatus};
use rupu_orchestrator::runs::RunStatus;

fn compact_run_id(id: &str) -> String {
    // Reuse the house compaction if available; fall back to a head…tail.
    crate::output::ids::compact_id(id)
}

/// Severity display order + short labels. Anything not listed here sorts
/// after these, alphabetically, under its own uppercased key.
const SEVERITY_RANK: [(&str, &str); 5] = [
    ("critical", "CRIT"),
    ("high", "HIGH"),
    ("medium", "MED"),
    ("low", "LOW"),
    ("info", "INFO"),
];

/// `findings N · n CRIT · n HIGH · …` in severity order, or `None` when the
/// run has no findings (the line is omitted entirely, never printed as 0).
fn findings_line(by_severity: &BTreeMap<String, usize>) -> Option<String> {
    if by_severity.is_empty() {
        return None;
    }
    let total: usize = by_severity.values().sum();
    let mut parts = vec![format!("findings {total}")];
    for (key, label) in SEVERITY_RANK {
        if let Some(n) = by_severity.get(key) {
            parts.push(format!("{n} {label}"));
        }
    }
    for (key, n) in by_severity {
        if !SEVERITY_RANK.iter().any(|(k, _)| k == key) {
            parts.push(format!("{n} {}", key.to_uppercase()));
        }
    }
    Some(parts.join(" · "))
}

pub fn render_completion_summary(v: &RunView, now: DateTime<Utc>) -> String {
    let mut out = String::new();
    let elapsed = v.elapsed_ms(now).map(fmt_hms).unwrap_or_default();

    if v.status == RunStatus::AwaitingApproval {
        let id = compact_run_id(&v.run_id);
        out.push_str(&format!(
            "{} {} · {} awaiting approval · {}\n",
            Status::Awaiting.glyph(),
            v.workflow_name,
            v.crew.as_deref().unwrap_or("-"),
            elapsed,
        ));
        // A view built from events alone may not have gates populated;
        // stay honest and point at the run rather than inventing a gate.
        if v.gates.is_empty() {
            out.push_str(&format!(
                "  awaiting approval · rupu workflow show-run {id}\n"
            ));
            return out;
        }
        for g in &v.gates {
            out.push_str(&format!(
                "  gate · {} — {}\n",
                g.step_id,
                g.prompt.as_deref().unwrap_or("approve to continue"),
            ));
        }
        if v.gates.len() == 1 {
            out.push_str(&format!(
                "  approve  rupu workflow approve {id}\n  reject   rupu workflow reject {id}\n"
            ));
        } else {
            // With more than one parked gate a bare approve/reject is
            // ambiguous at the CLI (`--gate <step_id>` is required), so
            // give every gate its own unambiguous command pair.
            for g in &v.gates {
                out.push_str(&format!(
                    "  approve  rupu workflow approve {id} --gate {step}\n  reject   rupu workflow reject {id} --gate {step}\n",
                    step = g.step_id,
                ));
            }
        }
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
        "{} {} · {}    {} · {}\n",
        status_glyph,
        v.workflow_name,
        v.crew.as_deref().unwrap_or("-"),
        v.status.as_str(),
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
    if let Some(line) = findings_line(&v.findings_by_severity) {
        out.push_str(&line);
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
        // one completed linear step
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

    fn gate(step_id: &str, prompt: Option<&str>) -> GateView {
        GateView {
            step_id: step_id.into(),
            prompt: prompt.map(str::to_string),
            since: Utc.with_ymd_and_hms(2026, 9, 30, 11, 30, 0).unwrap(),
            expires_at: Some(Utc.with_ymd_and_hms(2026, 10, 1, 11, 30, 0).unwrap()),
        }
    }

    fn awaiting_view(gates: Vec<GateView>) -> RunView {
        let mut v = RunView::default();
        v.run_id = "run_01M1SSABCDEFG".into();
        v.workflow_name = "assess-services".into();
        v.crew = Some("mint-tundra".into());
        v.status = RunStatus::AwaitingApproval;
        v.started_at = Some(Utc.with_ymd_and_hms(2026, 9, 30, 10, 12, 40).unwrap());
        v.gates = gates;
        v
    }

    fn unit(index: usize, key: &str, codename: Option<&str>, status: UnitStatus) -> UnitView {
        UnitView {
            index,
            unit_key: key.into(),
            agent: None,
            codename: codename.map(str::to_string),
            provider: None,
            model: None,
            host: None,
            status,
        }
    }

    /// A failed run: one clean linear step + a fan-out with 1 done / 2 failed
    /// units (one carrying a codename), plus findings across four severities.
    fn failed_fanout_view() -> RunView {
        let mut v = completed_view();
        v.status = RunStatus::Failed;
        let step = v.step_mut("sweep");
        step.kind = StepKind::ForEach;
        step.state = StepState::Failed;
        step.units
            .insert(0, unit(0, "svc-a", Some("heron#1"), UnitStatus::Done));
        step.units
            .insert(1, unit(1, "svc-b", Some("otter#7"), UnitStatus::Failed));
        step.units
            .insert(2, unit(2, "svc-c", None, UnitStatus::Failed));
        // Inserted out of rank order on purpose: output must be ranked, not
        // alphabetical (alphabetical would put `low` before `medium`).
        v.findings_by_severity.insert("low".into(), 2);
        v.findings_by_severity.insert("medium".into(), 3);
        v.findings_by_severity.insert("high".into(), 5);
        v.findings_by_severity.insert("critical".into(), 1);
        v
    }

    #[test]
    fn awaiting_gate_summary_snapshot() {
        let v = awaiting_view(vec![gate("triage", Some("approve report publish?"))]);
        let s = render_completion_summary(&v, now());
        insta::assert_snapshot!(s);
        // The run did not finish: no completion header/body.
        assert!(
            !s.contains("steps "),
            "gate block must not print steps:\n{s}"
        );
        assert!(
            !s.contains("tokens"),
            "gate block must not print tokens:\n{s}"
        );
        assert!(
            !s.contains("completed"),
            "gate block is not a completion:\n{s}"
        );
        assert!(s.contains("rupu workflow approve"), "{s}");
        // Single gate: a bare approve/reject is unambiguous.
        assert!(!s.contains("--gate"), "single gate needs no --gate:\n{s}");
    }

    #[test]
    fn awaiting_gate_prompt_none_falls_back() {
        let v = awaiting_view(vec![gate("triage", None)]);
        let s = render_completion_summary(&v, now());
        assert!(s.contains("approve to continue"), "{s}");
    }

    #[test]
    fn multi_gate_hint_uses_gate_flag() {
        let v = awaiting_view(vec![gate("triage", None), gate("publish", None)]);
        let s = render_completion_summary(&v, now());
        for step in ["triage", "publish"] {
            for verb in ["approve", "reject"] {
                let line = s
                    .lines()
                    .find(|l| {
                        l.trim_start().starts_with(verb) && l.contains(&format!("--gate {step}"))
                    })
                    .unwrap_or_else(|| panic!("no `{verb} --gate {step}` line in:\n{s}"));
                assert!(line.contains(&format!("rupu workflow {verb} ")), "{line}");
            }
        }
        // No bare (ambiguous) approve/reject line when >1 gate is parked.
        for line in s.lines() {
            let t = line.trim_start();
            if t.starts_with("approve") || t.starts_with("reject") {
                assert!(line.contains("--gate "), "ambiguous bare hint: {line}");
            }
        }
    }

    #[test]
    fn awaiting_without_gates_is_generic() {
        let v = awaiting_view(Vec::new());
        let s = render_completion_summary(&v, now());
        assert!(
            s.contains("awaiting approval · rupu workflow show-run run_01M1SSABCDEFG"),
            "{s}"
        );
        assert!(
            !s.contains("rupu workflow approve"),
            "no gate to approve:\n{s}"
        );
    }

    #[test]
    fn findings_and_failed_units_snapshot() {
        let s = render_completion_summary(&failed_fanout_view(), now());
        insta::assert_snapshot!(s);
        assert!(s.contains("units   1 ok · 2 failed"), "{s}");
        assert!(
            s.contains("sweep (1/3)"),
            "fan-out (done/total) annotation:\n{s}"
        );
        assert!(
            s.contains("findings 11 · 1 CRIT · 5 HIGH · 3 MED · 2 LOW"),
            "ranked findings line:\n{s}"
        );
        assert!(
            s.contains("failed  otter#7 (svc-b)  svc-c  → resume to retry"),
            "codename when present, unit key otherwise:\n{s}"
        );
        assert!(s.contains("failed · 1h 47m"), "lowercase status word:\n{s}");
    }

    #[test]
    fn findings_line_absent_when_empty() {
        let s = render_completion_summary(&completed_view(), now());
        assert!(!s.contains("findings"), "{s}");
    }

    #[test]
    fn findings_unrecognized_severities_follow_ranked_alphabetically() {
        let mut m = BTreeMap::new();
        m.insert("zeta".to_string(), 1);
        m.insert("info".to_string(), 4);
        m.insert("alpha".to_string(), 2);
        m.insert("high".to_string(), 3);
        assert_eq!(
            findings_line(&m).as_deref(),
            Some("findings 10 · 3 HIGH · 4 INFO · 2 ALPHA · 1 ZETA")
        );
        assert_eq!(findings_line(&BTreeMap::new()), None);
    }
}

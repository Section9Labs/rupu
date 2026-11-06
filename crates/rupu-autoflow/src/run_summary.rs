//! A run summarized for the autoflow views: `rupu autoflow explain`'s
//! last-run lines, and the CLI's monitor and serve views.

use crate::format::{format_cost, format_count, format_duration, truncate_text};
use rupu_orchestrator::usage_summary::{summarize_run, UsageSummary};
use rupu_orchestrator::{RunStore, StepResultRecord};
use rupu_transcript::event::{Event as TranscriptEvent, FileEditKind};
use rupu_transcript::reader::JsonlReader;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// How a step of a run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepOutcome {
    Complete,
    Skipped,
    Failed,
}

impl StepOutcome {
    /// A recorded step result's outcome.
    pub fn of(step: &StepResultRecord) -> Self {
        if step.skipped {
            Self::Skipped
        } else if step.success {
            Self::Complete
        } else {
            Self::Failed
        }
    }
}

/// One step of a run summary.
#[derive(Debug, Clone)]
pub struct AutoflowRunStepSummary {
    pub step_id: String,
    pub status: StepOutcome,
    pub detail: Option<String>,
}

/// A run's priced spend.
#[derive(Debug, Clone)]
pub struct AutoflowUsageSummary {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_tokens: u64,
    pub total_tokens: u64,
    pub cost_usd: Option<f64>,
    pub cost_partial: bool,
}

/// What a run's workspace changed, from `git status` / `git diff`.
#[derive(Debug, Clone)]
pub struct AutoflowWorkspaceDiffSummary {
    pub files_changed: usize,
    pub created: usize,
    pub modified: usize,
    pub deleted: usize,
    pub renamed: usize,
    pub insertions: u64,
    pub deletions: u64,
    pub top_files: Vec<String>,
    pub merge_target: Option<String>,
}

/// A run as the autoflow views show it: its record, activity counts from
/// its step transcripts, its usage fold and its workspace diff.
#[derive(Debug, Clone)]
pub struct AutoflowRunSummary {
    pub run_id: String,
    pub workflow: String,
    pub status: String,
    pub awaiting_step: Option<String>,
    pub error: Option<String>,
    pub worker: Option<String>,
    pub backend: Option<String>,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub finished_at: Option<chrono::DateTime<chrono::Utc>>,
    pub duration_ms: Option<u64>,
    pub workspace: Option<String>,
    pub agents: Vec<String>,
    pub providers: Vec<String>,
    pub models: Vec<String>,
    pub assistant_messages: u64,
    pub tool_calls: u64,
    pub command_runs: u64,
    pub actions_emitted: u64,
    pub file_edit_events: u64,
    pub file_creates: u64,
    pub file_modifies: u64,
    pub file_deletes: u64,
    pub usage: Option<AutoflowUsageSummary>,
    pub diff: Option<AutoflowWorkspaceDiffSummary>,
    pub steps: Vec<AutoflowRunStepSummary>,
}

/// Activity counts folded over a run's step transcripts.
#[derive(Debug, Clone, Default)]
pub struct TranscriptSummaryAccumulator {
    pub agents: BTreeSet<String>,
    pub providers: BTreeSet<String>,
    pub models: BTreeSet<String>,
    pub assistant_messages: u64,
    pub tool_calls: u64,
    pub command_runs: u64,
    pub actions_emitted: u64,
    pub file_edit_events: u64,
    pub file_creates: u64,
    pub file_modifies: u64,
    pub file_deletes: u64,
}

/// `run_id`'s summary, `None` when its record can't be loaded.
pub fn load_run_summary(
    run_store: &RunStore,
    run_id: &str,
    pricing: &rupu_config::PricingConfig,
) -> Option<AutoflowRunSummary> {
    let record = run_store.load(run_id).ok()?;
    let rows = run_store.read_step_results(run_id).unwrap_or_default();
    let transcript = summarize_run_transcripts(&rows);
    let diff = summarize_workspace_diff(&record.workspace_path);
    // Spend is the run's usage fold — every ledger row (units, dispatched
    // sub-agents, compaction calls) plus a fold of any known transcript
    // without rows — not just the completed steps' transcripts.
    let usage = usage_summary_from_fold(&summarize_run(run_store, run_id, pricing));
    let duration_ms = run_duration_ms(&record);
    let workspace = Some(record.workspace_path.display().to_string());
    let run_id = record.id.clone();
    let workflow = record.workflow_name.clone();
    let status = record.status.as_str().to_string();
    let awaiting_step = record.awaiting_step_id.clone();
    let error = record.error_message.clone();
    let worker = record.worker_id.clone();
    let backend = record.backend_id.clone();
    let started_at = record.started_at;
    let finished_at = record.finished_at;
    let steps = rows
        .into_iter()
        .map(|row| {
            let status = StepOutcome::of(&row);
            let detail = step_summary_detail(&row);
            AutoflowRunStepSummary {
                step_id: row.step_id,
                status,
                detail,
            }
        })
        .collect::<Vec<_>>();
    Some(AutoflowRunSummary {
        run_id,
        workflow,
        status,
        awaiting_step,
        error,
        worker,
        backend,
        started_at,
        finished_at,
        duration_ms: Some(duration_ms),
        workspace,
        agents: transcript.agents.into_iter().collect(),
        providers: transcript.providers.into_iter().collect(),
        models: transcript.models.into_iter().collect(),
        assistant_messages: transcript.assistant_messages,
        tool_calls: transcript.tool_calls,
        command_runs: transcript.command_runs,
        actions_emitted: transcript.actions_emitted,
        file_edit_events: transcript.file_edit_events,
        file_creates: transcript.file_creates,
        file_modifies: transcript.file_modifies,
        file_deletes: transcript.file_deletes,
        usage,
        diff,
        steps,
    })
}

/// Activity counts (agents, models, messages, tool calls, edits) across a
/// run's step transcripts. Token spend is NOT tallied here — it comes from
/// the run's usage fold ([`usage_summary_from_fold`]).
pub fn summarize_run_transcripts(rows: &[StepResultRecord]) -> TranscriptSummaryAccumulator {
    let mut out = TranscriptSummaryAccumulator::default();
    let mut seen = BTreeSet::new();
    for row in rows {
        summarize_transcript_path(&row.transcript_path, &mut out, &mut seen);
        for item in &row.items {
            summarize_transcript_path(&item.transcript_path, &mut out, &mut seen);
        }
    }
    out
}

pub fn summarize_transcript_path(
    path: &Path,
    out: &mut TranscriptSummaryAccumulator,
    seen: &mut BTreeSet<PathBuf>,
) {
    if !seen.insert(path.to_path_buf()) {
        return;
    }

    let Ok(iter) = JsonlReader::iter(path) else {
        return;
    };
    for event in iter.flatten() {
        match event {
            TranscriptEvent::RunStart {
                agent,
                provider,
                model,
                ..
            } => {
                out.agents.insert(agent);
                out.providers.insert(provider);
                out.models.insert(model);
            }
            TranscriptEvent::AssistantMessage { content, .. } if !content.trim().is_empty() => {
                out.assistant_messages += 1;
            }
            TranscriptEvent::ToolCall { .. } => out.tool_calls += 1,
            TranscriptEvent::CommandRun { .. } => out.command_runs += 1,
            TranscriptEvent::ActionEmitted { .. } => out.actions_emitted += 1,
            TranscriptEvent::FileEdit { kind, .. } => {
                out.file_edit_events += 1;
                match kind {
                    FileEditKind::Create => out.file_creates += 1,
                    FileEditKind::Modify => out.file_modifies += 1,
                    FileEditKind::Delete => out.file_deletes += 1,
                }
            }
            TranscriptEvent::Usage {
                provider, model, ..
            } => {
                out.providers.insert(provider);
                out.models.insert(model);
            }
            _ => {}
        }
    }
}

/// A run's spend for the autoflow views, from its priced usage fold.
/// `None` when the run spent nothing yet. `cost_partial` = some rows were
/// priced and some were not.
pub fn usage_summary_from_fold(usage: &UsageSummary) -> Option<AutoflowUsageSummary> {
    if usage.total_tokens == 0 && usage.cached_tokens == 0 {
        return None;
    }
    Some(AutoflowUsageSummary {
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        cached_tokens: usage.cached_tokens,
        total_tokens: usage.total_tokens,
        cost_usd: usage.cost_usd,
        cost_partial: usage.cost_usd.is_some() && !usage.priced,
    })
}

pub fn summarize_workspace_diff(path: &Path) -> Option<AutoflowWorkspaceDiffSummary> {
    let status_out = std::process::Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["status", "--porcelain"])
        .output()
        .ok()?;
    if !status_out.status.success() {
        return None;
    }

    let mut created = 0usize;
    let mut modified = 0usize;
    let mut deleted = 0usize;
    let mut renamed = 0usize;
    let mut changed_files = BTreeSet::new();
    let mut top_files = Vec::new();
    let mut untracked = Vec::new();

    for line in String::from_utf8_lossy(&status_out.stdout).lines() {
        if line.len() < 4 {
            continue;
        }
        let status = &line[..2];
        let raw_path = line[3..].trim();
        let path_name = raw_path
            .split(" -> ")
            .last()
            .unwrap_or(raw_path)
            .trim()
            .to_string();
        if path_name.is_empty() {
            continue;
        }
        if changed_files.insert(path_name.clone()) && top_files.len() < 4 {
            top_files.push(path_name.clone());
        }

        let mut chars = status.chars();
        let left = chars.next().unwrap_or(' ');
        let right = chars.next().unwrap_or(' ');
        if status == "??" {
            created += 1;
            untracked.push(path.join(&path_name));
        } else if left == 'R' || right == 'R' {
            renamed += 1;
        } else if left == 'D' || right == 'D' {
            deleted += 1;
        } else if left == 'A' || right == 'A' {
            created += 1;
        } else {
            modified += 1;
        }
    }

    let numstat_out = std::process::Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["diff", "--numstat", "--find-renames", "HEAD", "--"])
        .output()
        .ok();
    let mut insertions = 0u64;
    let mut deletions = 0u64;
    if let Some(out) = numstat_out.filter(|out| out.status.success()) {
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            let mut parts = line.splitn(3, '\t');
            let added = parts.next().unwrap_or("0");
            let removed = parts.next().unwrap_or("0");
            insertions += added.parse::<u64>().unwrap_or(0);
            deletions += removed.parse::<u64>().unwrap_or(0);
        }
    }
    for file in untracked {
        insertions += count_file_lines(&file).unwrap_or(0);
    }

    Some(AutoflowWorkspaceDiffSummary {
        files_changed: changed_files.len(),
        created,
        modified,
        deleted,
        renamed,
        insertions,
        deletions,
        top_files,
        merge_target: detect_origin_default_branch(path),
    })
}

pub fn count_file_lines(path: &Path) -> Option<u64> {
    let content = std::fs::read_to_string(path).ok()?;
    Some(content.lines().count() as u64)
}

pub fn detect_origin_default_branch(path: &Path) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(path)
        .args([
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ])
        .output()
        .ok()?;
    if out.status.success() {
        let value = String::from_utf8(out.stdout).ok()?.trim().to_string();
        if let Some(branch) = value.strip_prefix("origin/") {
            if !branch.is_empty() {
                return Some(branch.to_string());
            }
        }
    }

    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["symbolic-ref", "--short", "HEAD"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let value = String::from_utf8(out.stdout).ok()?.trim().to_string();
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

pub fn run_duration_ms(record: &rupu_orchestrator::RunRecord) -> u64 {
    match record.finished_at {
        Some(finished_at) => (finished_at - record.started_at).num_milliseconds().max(0) as u64,
        None => (chrono::Utc::now() - record.started_at)
            .num_milliseconds()
            .max(0) as u64,
    }
}

pub fn step_summary_detail(step: &StepResultRecord) -> Option<String> {
    if step.skipped {
        return Some("skipped".into());
    }
    if !step.findings.is_empty() {
        return Some(format!("{} finding(s)", step.findings.len()));
    }
    if !step.items.is_empty() {
        let ok = step.items.iter().filter(|item| item.success).count();
        return Some(format!("{ok}/{} item(s) ok", step.items.len()));
    }
    let first_line = step
        .output
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    if first_line.is_empty() {
        return None;
    }
    Some(first_line.to_string())
}

pub fn completed_steps(summary: &AutoflowRunSummary) -> usize {
    summary
        .steps
        .iter()
        .filter(|step| matches!(step.status, StepOutcome::Complete | StepOutcome::Skipped))
        .count()
}

pub fn format_run_models_line(summary: &AutoflowRunSummary) -> String {
    let agents = if summary.agents.is_empty() {
        "—".into()
    } else {
        truncate_text(&summary.agents.join(", "), 28)
    };
    let providers = if summary.providers.is_empty() {
        "—".into()
    } else {
        truncate_text(&summary.providers.join(", "), 24)
    };
    let models = if summary.models.is_empty() {
        "—".into()
    } else {
        truncate_text(&summary.models.join(", "), 36)
    };
    format!("agents {agents}  ·  providers {providers}  ·  models {models}")
}

pub fn format_run_usage_line(summary: &AutoflowRunSummary) -> String {
    let counts = format!(
        "messages {}  ·  tools {}  ·  commands {}  ·  actions {}",
        format_count(summary.assistant_messages),
        format_count(summary.tool_calls),
        format_count(summary.command_runs),
        format_count(summary.actions_emitted),
    );
    match &summary.usage {
        Some(usage) => format!(
            "{counts}  ·  tokens {} in / {} out / {} cached  ·  total {}  ·  cost {}",
            format_count(usage.input_tokens),
            format_count(usage.output_tokens),
            format_count(usage.cached_tokens),
            format_count(usage.total_tokens),
            format_cost(usage.cost_usd, usage.cost_partial),
        ),
        None => counts,
    }
}

pub fn format_run_diff_line(summary: &AutoflowRunSummary) -> String {
    let edit_counts = format!(
        "edit events {}  ·  creates {}  ·  modifies {}  ·  deletes {}",
        format_count(summary.file_edit_events),
        format_count(summary.file_creates),
        format_count(summary.file_modifies),
        format_count(summary.file_deletes),
    );
    match &summary.diff {
        Some(diff) => {
            let mut parts = vec![format!(
                "workspace {} file(s)  ·  {} new  ·  {} modified  ·  {} deleted  ·  {} renamed  ·  +{}/-{}",
                diff.files_changed,
                diff.created,
                diff.modified,
                diff.deleted,
                diff.renamed,
                format_count(diff.insertions),
                format_count(diff.deletions),
            )];
            if !diff.top_files.is_empty() {
                parts.push(format!(
                    "files {}",
                    truncate_text(&diff.top_files.join(", "), 44)
                ));
            }
            if let Some(target) = diff.merge_target.as_deref() {
                parts.push(format!("merge -> {target}"));
            }
            format!("{edit_counts}  ·  {}", parts.join("  ·  "))
        }
        None => edit_counts,
    }
}

pub fn format_run_metrics_line(summary: &AutoflowRunSummary) -> String {
    let mut parts = vec![format!(
        "steps {}/{}",
        completed_steps(summary),
        summary.steps.len()
    )];
    if let Some(duration_ms) = summary.duration_ms {
        parts.push(format!(
            "duration {}",
            format_duration(std::time::Duration::from_millis(duration_ms))
        ));
    }
    parts.push(format!("tools {}", format_count(summary.tool_calls)));
    if let Some(usage) = &summary.usage {
        parts.push(format!("tokens {}", format_count(usage.total_tokens)));
        parts.push(format!(
            "cost {}",
            format_cost(usage.cost_usd, usage.cost_partial)
        ));
    }
    if let Some(diff) = &summary.diff {
        parts.push(format!(
            "diff {} file(s) +{}/-{}",
            diff.files_changed,
            format_count(diff.insertions),
            format_count(diff.deletions)
        ));
    }
    parts.join("  ·  ")
}

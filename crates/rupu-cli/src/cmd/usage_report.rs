use chrono::{DateTime, Utc};
use rupu_orchestrator::{RunRecord, RunStore, StepResultRecord};
use rupu_runtime::RunEnvelope;
use rupu_transcript::{JsonlReader, TimeWindow, UsageRow};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageSource {
    StandaloneRun,
    WorkflowRun,
}

#[derive(Debug, Clone, Serialize)]
pub struct UsageFact {
    pub source: UsageSource,
    pub run_id: String,
    pub started_at: DateTime<Utc>,
    pub status: String,
    pub workflow_name: Option<String>,
    pub repo_ref: Option<String>,
    pub issue_ref: Option<String>,
    pub worker_id: Option<String>,
    pub backend_id: Option<String>,
    pub trigger_source: Option<String>,
    pub provider: String,
    pub model: String,
    pub agent: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_tokens: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct UsageRun {
    pub source: UsageSource,
    pub run_id: String,
    pub started_at: DateTime<Utc>,
    pub status: String,
    pub workflow_name: Option<String>,
    pub repo_ref: Option<String>,
    pub issue_ref: Option<String>,
    pub worker_id: Option<String>,
    pub backend_id: Option<String>,
    pub trigger_source: Option<String>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_tokens: u64,
    pub providers: Vec<String>,
    pub models: Vec<String>,
    pub agents: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct UsageTotals {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_tokens: u64,
    pub runs: u64,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct UsageDataset {
    pub facts: Vec<UsageFact>,
    pub runs: Vec<UsageRun>,
}

impl UsageDataset {
    pub fn load(
        global_root: &Path,
        project_root: Option<&Path>,
        window: TimeWindow,
    ) -> anyhow::Result<Self> {
        let run_store = RunStore::new(global_root.join("runs"));
        let workflow_runs = run_store.list()?;
        let referenced_paths = referenced_transcript_paths(&run_store, &workflow_runs);

        let mut facts = Vec::new();
        let mut runs = Vec::new();

        for run in workflow_runs {
            let metadata = WorkflowUsageMetadata::from_run_record(&run, &run_store);
            let transcript_paths = transcript_paths_for_run(&run_store, &run.id);
            let rows = rupu_transcript::aggregate(&transcript_paths, TimeWindow::default());
            let totals = usage_totals(&rows);
            if window_contains(window, run.started_at) {
                facts.extend(rows.iter().map(|row| metadata.to_fact(row)));
                runs.push(metadata.to_run(totals, &rows));
            }
        }

        let mut standalone_paths = BTreeSet::new();
        if let Some(project_root) = project_root {
            collect_jsonl(
                &project_root.join(".rupu/transcripts"),
                &mut standalone_paths,
            );
        }
        collect_jsonl(&global_root.join("transcripts"), &mut standalone_paths);

        for path in standalone_paths {
            if referenced_paths.contains(&path) {
                continue;
            }
            let Ok(summary) = JsonlReader::summary(&path) else {
                continue;
            };
            if !window_contains(window, summary.started_at) {
                continue;
            }
            let rows =
                rupu_transcript::aggregate(std::slice::from_ref(&path), TimeWindow::default());
            let metadata = StandaloneUsageMetadata::from_summary(&summary);
            let totals = usage_totals(&rows);
            facts.extend(rows.iter().map(|row| metadata.to_fact(row)));
            runs.push(metadata.to_run(totals, &rows));
        }

        runs.sort_by_key(|row| std::cmp::Reverse(row.started_at));
        Ok(Self { facts, runs })
    }

    pub fn composite_rows(&self) -> Vec<UsageRow> {
        let mut grouped: BTreeMap<(String, String, String), UsageRow> = BTreeMap::new();
        let mut run_ids_by_key: BTreeMap<(String, String, String), BTreeSet<String>> =
            BTreeMap::new();
        for fact in &self.facts {
            let key = (
                fact.provider.clone(),
                fact.model.clone(),
                fact.agent.clone(),
            );
            let entry = grouped.entry(key.clone()).or_insert_with(|| UsageRow {
                provider: fact.provider.clone(),
                model: fact.model.clone(),
                agent: fact.agent.clone(),
                ..UsageRow::default()
            });
            entry.input_tokens += fact.input_tokens;
            entry.output_tokens += fact.output_tokens;
            entry.cached_tokens += fact.cached_tokens;
            run_ids_by_key
                .entry(key)
                .or_default()
                .insert(fact.run_id.clone());
        }
        for (key, run_ids) in run_ids_by_key {
            if let Some(row) = grouped.get_mut(&key) {
                row.runs = run_ids.len() as u64;
            }
        }
        let mut rows = grouped.into_values().collect::<Vec<_>>();
        rows.sort_by(|a, b| {
            (b.input_tokens + b.output_tokens)
                .cmp(&(a.input_tokens + a.output_tokens))
                .then_with(|| {
                    (a.provider.as_str(), a.model.as_str(), a.agent.as_str()).cmp(&(
                        b.provider.as_str(),
                        b.model.as_str(),
                        b.agent.as_str(),
                    ))
                })
        });
        rows
    }
}

fn referenced_transcript_paths(store: &RunStore, runs: &[RunRecord]) -> BTreeSet<PathBuf> {
    let mut paths = BTreeSet::new();
    for run in runs {
        for path in transcript_paths_for_run(store, &run.id) {
            paths.insert(canonicalize_path(path));
        }
    }
    paths
}

fn transcript_paths_for_run(store: &RunStore, run_id: &str) -> Vec<PathBuf> {
    let Ok(records) = store.read_step_results(run_id) else {
        return Vec::new();
    };
    transcript_paths_from_records(&records)
}

fn transcript_paths_from_records(records: &[StepResultRecord]) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for record in records {
        paths.push(record.transcript_path.clone());
        for item in &record.items {
            paths.push(item.transcript_path.clone());
        }
    }
    paths
}

fn collect_jsonl(dir: &Path, out: &mut BTreeSet<PathBuf>) {
    let Ok(read_dir) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in read_dir.flatten() {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) == Some("jsonl") {
            out.insert(canonicalize_path(path));
        }
    }
}

fn canonicalize_path(path: PathBuf) -> PathBuf {
    path.canonicalize().unwrap_or(path)
}

fn usage_totals(rows: &[UsageRow]) -> UsageTotals {
    UsageTotals {
        input_tokens: rows.iter().map(|row| row.input_tokens).sum(),
        output_tokens: rows.iter().map(|row| row.output_tokens).sum(),
        cached_tokens: rows.iter().map(|row| row.cached_tokens).sum(),
        runs: 1,
    }
}

fn window_contains(window: TimeWindow, ts: DateTime<Utc>) -> bool {
    if let Some(since) = window.since {
        if ts < since {
            return false;
        }
    }
    if let Some(until) = window.until {
        if ts > until {
            return false;
        }
    }
    true
}

fn distinct(values: impl Iterator<Item = String>) -> Vec<String> {
    values.collect::<BTreeSet<_>>().into_iter().collect()
}

fn repo_ref_from_issue_ref(issue_ref: Option<&str>) -> Option<String> {
    issue_ref.and_then(|value| {
        value
            .split_once("/issues/")
            .map(|(prefix, _)| prefix.to_string())
    })
}

struct WorkflowUsageMetadata {
    run_id: String,
    started_at: DateTime<Utc>,
    status: String,
    workflow_name: Option<String>,
    repo_ref: Option<String>,
    issue_ref: Option<String>,
    worker_id: Option<String>,
    backend_id: Option<String>,
    trigger_source: Option<String>,
}

impl WorkflowUsageMetadata {
    fn from_run_record(run: &RunRecord, store: &RunStore) -> Self {
        let envelope = store.read_run_envelope(&run.id).ok();
        let repo_ref = envelope
            .as_ref()
            .and_then(|value| value.repo.as_ref())
            .and_then(|repo| repo.repo_ref.clone())
            .or_else(|| repo_ref_from_issue_ref(run.issue_ref.as_deref()));
        let issue_ref = envelope
            .as_ref()
            .and_then(|value| value.context.as_ref())
            .and_then(|context| context.issue_ref.clone())
            .or_else(|| run.issue_ref.clone());
        Self {
            run_id: run.id.clone(),
            started_at: run.started_at,
            status: run.status.as_str().to_string(),
            workflow_name: Some(run.workflow_name.clone()),
            repo_ref,
            issue_ref,
            worker_id: run.worker_id.clone(),
            backend_id: run.backend_id.clone(),
            trigger_source: envelope.as_ref().map(trigger_name),
        }
    }

    fn to_fact(&self, row: &UsageRow) -> UsageFact {
        UsageFact {
            source: UsageSource::WorkflowRun,
            run_id: self.run_id.clone(),
            started_at: self.started_at,
            status: self.status.clone(),
            workflow_name: self.workflow_name.clone(),
            repo_ref: self.repo_ref.clone(),
            issue_ref: self.issue_ref.clone(),
            worker_id: self.worker_id.clone(),
            backend_id: self.backend_id.clone(),
            trigger_source: self.trigger_source.clone(),
            provider: row.provider.clone(),
            model: row.model.clone(),
            agent: row.agent.clone(),
            input_tokens: row.input_tokens,
            output_tokens: row.output_tokens,
            cached_tokens: row.cached_tokens,
        }
    }

    fn to_run(&self, totals: UsageTotals, rows: &[UsageRow]) -> UsageRun {
        UsageRun {
            source: UsageSource::WorkflowRun,
            run_id: self.run_id.clone(),
            started_at: self.started_at,
            status: self.status.clone(),
            workflow_name: self.workflow_name.clone(),
            repo_ref: self.repo_ref.clone(),
            issue_ref: self.issue_ref.clone(),
            worker_id: self.worker_id.clone(),
            backend_id: self.backend_id.clone(),
            trigger_source: self.trigger_source.clone(),
            input_tokens: totals.input_tokens,
            output_tokens: totals.output_tokens,
            cached_tokens: totals.cached_tokens,
            providers: distinct(rows.iter().map(|row| row.provider.clone())),
            models: distinct(rows.iter().map(|row| row.model.clone())),
            agents: distinct(rows.iter().map(|row| row.agent.clone())),
        }
    }
}

struct StandaloneUsageMetadata {
    run_id: String,
    started_at: DateTime<Utc>,
    status: String,
    agent: String,
    provider: String,
    model: String,
}

impl StandaloneUsageMetadata {
    fn from_summary(summary: &rupu_transcript::RunSummary) -> Self {
        Self {
            run_id: summary.run_id.clone(),
            started_at: summary.started_at,
            status: match summary.status {
                rupu_transcript::RunStatus::Ok => "completed".into(),
                rupu_transcript::RunStatus::Error => "failed".into(),
                rupu_transcript::RunStatus::Aborted => "aborted".into(),
            },
            agent: summary.agent.clone(),
            provider: summary.provider.clone(),
            model: summary.model.clone(),
        }
    }

    fn to_fact(&self, row: &UsageRow) -> UsageFact {
        UsageFact {
            source: UsageSource::StandaloneRun,
            run_id: self.run_id.clone(),
            started_at: self.started_at,
            status: self.status.clone(),
            workflow_name: None,
            repo_ref: None,
            issue_ref: None,
            worker_id: None,
            backend_id: None,
            trigger_source: Some("standalone_run".into()),
            provider: row.provider.clone(),
            model: row.model.clone(),
            agent: row.agent.clone(),
            input_tokens: row.input_tokens,
            output_tokens: row.output_tokens,
            cached_tokens: row.cached_tokens,
        }
    }

    fn to_run(&self, totals: UsageTotals, rows: &[UsageRow]) -> UsageRun {
        UsageRun {
            source: UsageSource::StandaloneRun,
            run_id: self.run_id.clone(),
            started_at: self.started_at,
            status: self.status.clone(),
            workflow_name: None,
            repo_ref: None,
            issue_ref: None,
            worker_id: None,
            backend_id: None,
            trigger_source: Some("standalone_run".into()),
            input_tokens: totals.input_tokens,
            output_tokens: totals.output_tokens,
            cached_tokens: totals.cached_tokens,
            providers: distinct(
                rows.iter()
                    .map(|row| row.provider.clone())
                    .chain(std::iter::once(self.provider.clone())),
            ),
            models: distinct(
                rows.iter()
                    .map(|row| row.model.clone())
                    .chain(std::iter::once(self.model.clone())),
            ),
            agents: distinct(
                rows.iter()
                    .map(|row| row.agent.clone())
                    .chain(std::iter::once(self.agent.clone())),
            ),
        }
    }
}

fn trigger_name(envelope: &RunEnvelope) -> String {
    match envelope.trigger.source {
        rupu_runtime::RunTriggerSource::WorkflowCli => "workflow_cli",
        rupu_runtime::RunTriggerSource::IssueCommand => "issue_command",
        rupu_runtime::RunTriggerSource::EventDispatch => "event_dispatch",
        rupu_runtime::RunTriggerSource::CronEvent => "cron_event",
        rupu_runtime::RunTriggerSource::Autoflow => "autoflow",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use rupu_orchestrator::{RunStatus, StepKind, StepResultRecord};
    use rupu_runtime::{
        ExecutionRequest, RepoBinding, RunContext, RunEnvelope, RunKind, RunTrigger,
        RunTriggerSource, WorkflowBinding,
    };
    use rupu_transcript::{Event, JsonlWriter};

    #[allow(clippy::too_many_arguments)]
    fn write_usage_transcript(
        dir: &Path,
        run_id: &str,
        agent: &str,
        provider: &str,
        model: &str,
        started_at: DateTime<Utc>,
        input_tokens: u32,
        output_tokens: u32,
    ) -> PathBuf {
        let path = dir.join(format!("{run_id}.jsonl"));
        let mut writer = JsonlWriter::create(&path).unwrap();
        writer
            .write(&Event::RunStart {
                run_id: run_id.into(),
                workspace_id: "ws".into(),
                agent: agent.into(),
                provider: provider.into(),
                model: model.into(),
                started_at,
                mode: rupu_transcript::RunMode::Bypass,
            })
            .unwrap();
        writer
            .write(&Event::Usage {
                provider: provider.into(),
                model: model.into(),
                input_tokens,
                output_tokens,
                cached_tokens: 0,
            })
            .unwrap();
        writer
            .write(&Event::RunComplete {
                run_id: run_id.into(),
                status: rupu_transcript::RunStatus::Ok,
                total_tokens: (input_tokens + output_tokens) as u64,
                duration_ms: 100,
                error: None,
            })
            .unwrap();
        writer.flush().unwrap();
        path
    }

    fn sample_run_record(id: &str, started_at: DateTime<Utc>, transcript_dir: &Path) -> RunRecord {
        RunRecord {
            id: id.into(),
            workflow_name: "phase-delivery-cycle".into(),
            status: RunStatus::Completed,
            inputs: BTreeMap::new(),
            event: None,
            workspace_id: "ws_01".into(),
            workspace_path: PathBuf::from("/tmp/repo"),
            transcript_dir: transcript_dir.to_path_buf(),
            started_at,
            finished_at: Some(started_at + Duration::minutes(5)),
            error_message: None,
            awaiting_step_id: None,
            approval_prompt: None,
            awaiting_since: None,
            expires_at: None,
            issue_ref: Some("github:Section9Labs/rupu/issues/42".into()),
            issue: None,
            parent_run_id: None,
            backend_id: Some("local_worktree".into()),
            worker_id: Some("worker_local_cli".into()),
            artifact_manifest_path: None,
            source_wake_id: None,
        }
    }

    fn sample_envelope(id: &str) -> RunEnvelope {
        RunEnvelope {
            version: RunEnvelope::VERSION,
            run_id: id.into(),
            kind: RunKind::WorkflowRun,
            workflow: WorkflowBinding {
                name: "phase-delivery-cycle".into(),
                source_path: PathBuf::from(".rupu/workflows/phase-delivery-cycle.yaml"),
                fingerprint: "sha256:test".into(),
            },
            repo: Some(RepoBinding {
                repo_ref: Some("github:Section9Labs/rupu".into()),
                project_root: Some(PathBuf::from("/tmp/repo")),
                workspace_id: "ws_01".into(),
                workspace_path: PathBuf::from("/tmp/repo"),
            }),
            trigger: RunTrigger {
                source: RunTriggerSource::Autoflow,
                wake_id: Some("wake_01".into()),
                event_id: Some("github.issue.opened".into()),
            },
            inputs: BTreeMap::new(),
            context: Some(RunContext {
                issue_ref: Some("github:Section9Labs/rupu/issues/42".into()),
                target: Some("github:Section9Labs/rupu/issues/42".into()),
                event_present: true,
                issue_present: true,
            }),
            execution: ExecutionRequest {
                backend: Some("local_worktree".into()),
                permission_mode: "bypass".into(),
                workspace_strategy: Some("managed_worktree".into()),
                strict_templates: true,
                attach_ui: false,
                use_canvas: false,
            },
            autoflow: None,
            correlation: None,
            worker: None,
        }
    }

    fn sample_step_result(transcript_path: &Path) -> StepResultRecord {
        StepResultRecord {
            step_id: "implement".into(),
            run_id: "run_01".into(),
            transcript_path: transcript_path.to_path_buf(),
            output: "ok".into(),
            success: true,
            skipped: false,
            rendered_prompt: "do work".into(),
            kind: StepKind::Linear,
            items: Vec::new(),
            findings: Vec::new(),
            iterations: 0,
            resolved: true,
            finished_at: Utc::now(),
        }
    }

    #[test]
    fn dataset_loads_standalone_run_usage() {
        let temp = tempfile::tempdir().unwrap();
        let global = temp.path().join(".rupu");
        let transcripts = global.join("transcripts");
        std::fs::create_dir_all(&transcripts).unwrap();
        let started_at = Utc::now();
        write_usage_transcript(
            &transcripts,
            "run_standalone_01",
            "reviewer",
            "anthropic",
            "claude-sonnet-4-6",
            started_at,
            10,
            4,
        );

        let dataset = UsageDataset::load(&global, None, TimeWindow::default()).unwrap();
        assert_eq!(dataset.runs.len(), 1);
        assert_eq!(dataset.facts.len(), 1);
        assert_eq!(dataset.runs[0].source, UsageSource::StandaloneRun);
        assert_eq!(dataset.runs[0].providers, vec!["anthropic"]);
        assert_eq!(dataset.facts[0].agent, "reviewer");
        assert_eq!(dataset.facts[0].input_tokens, 10);
        assert_eq!(dataset.runs.len(), 1);
    }

    #[test]
    fn dataset_loads_workflow_run_usage_without_double_counting_step_transcript() {
        let temp = tempfile::tempdir().unwrap();
        let global = temp.path().join(".rupu");
        let transcripts = global.join("transcripts");
        let runs_root = global.join("runs");
        std::fs::create_dir_all(&transcripts).unwrap();
        std::fs::create_dir_all(&runs_root).unwrap();

        let started_at = Utc::now();
        let transcript_path = write_usage_transcript(
            &transcripts,
            "step_run_01",
            "implementer",
            "openai",
            "gpt-5",
            started_at,
            30,
            10,
        );

        let store = RunStore::new(runs_root);
        store
            .write_run_envelope("run_workflow_01", &sample_envelope("run_workflow_01"))
            .unwrap();
        store
            .create(
                sample_run_record("run_workflow_01", started_at, &transcripts),
                "name: phase-delivery-cycle\nsteps: []\n",
            )
            .unwrap();
        store
            .append_step_result("run_workflow_01", &sample_step_result(&transcript_path))
            .unwrap();

        let dataset = UsageDataset::load(&global, None, TimeWindow::default()).unwrap();
        assert_eq!(
            dataset.runs.len(),
            1,
            "workflow transcript should not be counted twice"
        );
        assert_eq!(
            dataset.facts.len(),
            1,
            "workflow transcript should not be counted twice"
        );
        assert_eq!(dataset.runs[0].source, UsageSource::WorkflowRun);
        assert_eq!(
            dataset.runs[0].repo_ref.as_deref(),
            Some("github:Section9Labs/rupu")
        );
        assert_eq!(
            dataset.runs[0].workflow_name.as_deref(),
            Some("phase-delivery-cycle")
        );
        assert_eq!(
            dataset.runs[0].worker_id.as_deref(),
            Some("worker_local_cli")
        );
        assert_eq!(dataset.facts[0].provider, "openai");
        assert_eq!(dataset.facts[0].input_tokens, 30);
        assert_eq!(dataset.facts[0].output_tokens, 10);
    }
}

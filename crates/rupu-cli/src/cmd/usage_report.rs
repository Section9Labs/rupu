use chrono::{DateTime, Utc};
use rupu_orchestrator::{RunRecord, RunStore, StepResultRecord};
use rupu_runtime::RunEnvelope;
use rupu_transcript::{JsonlReader, TimeWindow, UsageRow};
use rupu_workspace::WorkspaceStore;
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

#[derive(Debug, Clone, Default)]
pub struct UsageFilter {
    pub repo_ref: Option<String>,
    pub issue_ref: Option<String>,
    pub workflow_name: Option<String>,
    pub agent: Option<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub worker_id: Option<String>,
    pub backend_id: Option<String>,
    pub trigger_source: Option<String>,
    pub status: Option<String>,
    pub failed_only: bool,
}

impl UsageFilter {
    fn matches_fact(&self, fact: &UsageFact) -> bool {
        if self.failed_only && fact.status != "failed" {
            return false;
        }
        if self
            .status
            .as_deref()
            .is_some_and(|status| fact.status != status)
        {
            return false;
        }
        if self
            .repo_ref
            .as_deref()
            .is_some_and(|repo_ref| fact.repo_ref.as_deref() != Some(repo_ref))
        {
            return false;
        }
        if self
            .issue_ref
            .as_deref()
            .is_some_and(|issue_ref| fact.issue_ref.as_deref() != Some(issue_ref))
        {
            return false;
        }
        if self
            .workflow_name
            .as_deref()
            .is_some_and(|workflow| fact.workflow_name.as_deref() != Some(workflow))
        {
            return false;
        }
        if self
            .agent
            .as_deref()
            .is_some_and(|agent| fact.agent != agent)
        {
            return false;
        }
        if self
            .provider
            .as_deref()
            .is_some_and(|provider| fact.provider != provider)
        {
            return false;
        }
        if self
            .model
            .as_deref()
            .is_some_and(|model| fact.model != model)
        {
            return false;
        }
        if self
            .worker_id
            .as_deref()
            .is_some_and(|worker_id| fact.worker_id.as_deref() != Some(worker_id))
        {
            return false;
        }
        if self
            .backend_id
            .as_deref()
            .is_some_and(|backend_id| fact.backend_id.as_deref() != Some(backend_id))
        {
            return false;
        }
        if self
            .trigger_source
            .as_deref()
            .is_some_and(|trigger| fact.trigger_source.as_deref() != Some(trigger))
        {
            return false;
        }
        true
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct UsageDataset {
    pub facts: Vec<UsageFact>,
    pub runs: Vec<UsageRun>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct StandaloneMetadataBackfillStats {
    pub scanned: u64,
    pub referenced_workflow_transcripts: u64,
    pub existing_sidecars: u64,
    pub backfilled: u64,
    pub unresolved_workspace: u64,
    pub unreadable_transcripts: u64,
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

        for run in workflow_runs {
            let metadata = WorkflowUsageMetadata::from_run_record(&run, &run_store);
            let transcript_paths = transcript_paths_for_run(&run_store, &run.id);
            let rows = rupu_transcript::aggregate(&transcript_paths, TimeWindow::default());
            if window_contains(window, run.started_at) {
                facts.extend(rows.iter().map(|row| metadata.to_fact(row)));
            }
        }

        let standalone_paths = standalone_transcript_paths(global_root, project_root);

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
            let metadata = StandaloneUsageMetadata::from_summary(
                &summary,
                load_standalone_metadata(&path, &summary.run_id),
            );
            facts.extend(rows.iter().map(|row| metadata.to_fact(row)));
        }

        let mut runs = build_runs(&facts);
        runs.sort_by_key(|row| std::cmp::Reverse(row.started_at));
        Ok(Self { facts, runs })
    }

    pub fn totals(&self) -> UsageTotals {
        UsageTotals {
            input_tokens: self.facts.iter().map(|fact| fact.input_tokens).sum(),
            output_tokens: self.facts.iter().map(|fact| fact.output_tokens).sum(),
            cached_tokens: self.facts.iter().map(|fact| fact.cached_tokens).sum(),
            runs: self
                .facts
                .iter()
                .map(|fact| fact.run_id.as_str())
                .collect::<BTreeSet<_>>()
                .len() as u64,
        }
    }

    pub fn filtered(&self, filter: &UsageFilter) -> Self {
        let facts = self
            .facts
            .iter()
            .filter(|fact| filter.matches_fact(fact))
            .cloned()
            .collect::<Vec<_>>();
        let mut runs = build_runs(&facts);
        runs.sort_by_key(|row| std::cmp::Reverse(row.started_at));
        Self { facts, runs }
    }
}

pub fn backfill_standalone_metadata(
    global_root: &Path,
    project_root: Option<&Path>,
    force: bool,
) -> anyhow::Result<StandaloneMetadataBackfillStats> {
    let run_store = RunStore::new(global_root.join("runs"));
    let workflow_runs = run_store.list()?;
    let referenced_paths = referenced_transcript_paths(&run_store, &workflow_runs);
    let workspace_store = WorkspaceStore {
        root: global_root.join("workspaces"),
    };
    let mut stats = StandaloneMetadataBackfillStats::default();

    for path in standalone_transcript_paths(global_root, project_root) {
        stats.scanned += 1;
        if referenced_paths.contains(&path) {
            stats.referenced_workflow_transcripts += 1;
            continue;
        }
        let Ok(summary) = JsonlReader::summary(&path) else {
            stats.unreadable_transcripts += 1;
            continue;
        };
        let metadata_path = load_standalone_metadata_path(&path, &summary.run_id)?;
        if metadata_path.exists() && !force {
            stats.existing_sidecars += 1;
            continue;
        }
        let Some(workspace) = workspace_store.load(&summary.workspace_id)? else {
            stats.unresolved_workspace += 1;
            continue;
        };
        let metadata = backfill_metadata_from_workspace(&summary.run_id, &workspace);
        crate::standalone_run_metadata::write_metadata(&metadata_path, &metadata)?;
        stats.backfilled += 1;
    }

    Ok(stats)
}

fn build_runs(facts: &[UsageFact]) -> Vec<UsageRun> {
    #[derive(Default)]
    struct RunAccumulator {
        source: Option<UsageSource>,
        run_id: String,
        started_at: Option<DateTime<Utc>>,
        status: String,
        workflow_name: Option<String>,
        repo_ref: Option<String>,
        issue_ref: Option<String>,
        worker_id: Option<String>,
        backend_id: Option<String>,
        trigger_source: Option<String>,
        input_tokens: u64,
        output_tokens: u64,
        cached_tokens: u64,
        providers: BTreeSet<String>,
        models: BTreeSet<String>,
        agents: BTreeSet<String>,
    }

    let mut grouped: BTreeMap<String, RunAccumulator> = BTreeMap::new();
    for fact in facts {
        let entry = grouped
            .entry(fact.run_id.clone())
            .or_insert_with(|| RunAccumulator {
                source: Some(fact.source),
                run_id: fact.run_id.clone(),
                started_at: Some(fact.started_at),
                status: fact.status.clone(),
                workflow_name: fact.workflow_name.clone(),
                repo_ref: fact.repo_ref.clone(),
                issue_ref: fact.issue_ref.clone(),
                worker_id: fact.worker_id.clone(),
                backend_id: fact.backend_id.clone(),
                trigger_source: fact.trigger_source.clone(),
                ..RunAccumulator::default()
            });
        entry.input_tokens += fact.input_tokens;
        entry.output_tokens += fact.output_tokens;
        entry.cached_tokens += fact.cached_tokens;
        entry.providers.insert(fact.provider.clone());
        entry.models.insert(fact.model.clone());
        entry.agents.insert(fact.agent.clone());
    }

    grouped
        .into_values()
        .filter_map(|entry| {
            Some(UsageRun {
                source: entry.source?,
                run_id: entry.run_id,
                started_at: entry.started_at?,
                status: entry.status,
                workflow_name: entry.workflow_name,
                repo_ref: entry.repo_ref,
                issue_ref: entry.issue_ref,
                worker_id: entry.worker_id,
                backend_id: entry.backend_id,
                trigger_source: entry.trigger_source,
                input_tokens: entry.input_tokens,
                output_tokens: entry.output_tokens,
                cached_tokens: entry.cached_tokens,
                providers: entry.providers.into_iter().collect(),
                models: entry.models.into_iter().collect(),
                agents: entry.agents.into_iter().collect(),
            })
        })
        .collect()
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

fn standalone_transcript_paths(
    global_root: &Path,
    project_root: Option<&Path>,
) -> BTreeSet<PathBuf> {
    let mut standalone_paths = BTreeSet::new();
    if let Some(project_root) = project_root {
        collect_jsonl(
            &project_root.join(".rupu/transcripts"),
            &mut standalone_paths,
        );
    }
    collect_jsonl(&global_root.join("transcripts"), &mut standalone_paths);
    standalone_paths
}

fn canonicalize_path(path: PathBuf) -> PathBuf {
    path.canonicalize().unwrap_or(path)
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
}

struct StandaloneUsageMetadata {
    run_id: String,
    started_at: DateTime<Utc>,
    status: String,
    repo_ref: Option<String>,
    issue_ref: Option<String>,
    worker_id: Option<String>,
    backend_id: Option<String>,
    trigger_source: Option<String>,
}

impl StandaloneUsageMetadata {
    fn from_summary(
        summary: &rupu_transcript::RunSummary,
        metadata: Option<crate::standalone_run_metadata::StandaloneRunMetadata>,
    ) -> Self {
        Self {
            run_id: summary.run_id.clone(),
            started_at: summary.started_at,
            status: match summary.status {
                rupu_transcript::RunStatus::Ok => "completed".into(),
                rupu_transcript::RunStatus::Error => "failed".into(),
                rupu_transcript::RunStatus::Aborted => "aborted".into(),
            },
            repo_ref: metadata.as_ref().and_then(|value| value.repo_ref.clone()),
            issue_ref: metadata.as_ref().and_then(|value| value.issue_ref.clone()),
            worker_id: metadata.as_ref().and_then(|value| value.worker_id.clone()),
            backend_id: metadata.as_ref().map(|value| value.backend_id.clone()),
            trigger_source: metadata.map(|value| value.trigger_source),
        }
    }

    fn to_fact(&self, row: &UsageRow) -> UsageFact {
        UsageFact {
            source: UsageSource::StandaloneRun,
            run_id: self.run_id.clone(),
            started_at: self.started_at,
            status: self.status.clone(),
            workflow_name: None,
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

fn load_standalone_metadata(
    transcript_path: &Path,
    run_id: &str,
) -> Option<crate::standalone_run_metadata::StandaloneRunMetadata> {
    let path = load_standalone_metadata_path(transcript_path, run_id).ok()?;
    crate::standalone_run_metadata::read_metadata(&path).ok()
}

fn load_standalone_metadata_path(transcript_path: &Path, run_id: &str) -> anyhow::Result<PathBuf> {
    let dir = transcript_path.parent().ok_or_else(|| {
        anyhow::anyhow!(
            "standalone transcript `{}` has no parent directory",
            transcript_path.display()
        )
    })?;
    Ok(crate::standalone_run_metadata::metadata_path_for_run(
        dir, run_id,
    ))
}

fn backfill_metadata_from_workspace(
    run_id: &str,
    workspace: &rupu_workspace::Workspace,
) -> crate::standalone_run_metadata::StandaloneRunMetadata {
    let workspace_path = PathBuf::from(&workspace.path);
    let repo_ref = crate::cmd::issues::autodetect_repo_from_path(&workspace_path)
        .ok()
        .map(|repo| crate::cmd::issues::canonical_repo_ref(&repo))
        .or_else(|| {
            workspace
                .repo_remote
                .as_deref()
                .and_then(crate::cmd::issues::parse_remote_url)
                .map(|repo| crate::cmd::issues::canonical_repo_ref(&repo))
        });
    let project_root = crate::paths::project_root_for(&workspace_path)
        .ok()
        .flatten()
        .or_else(|| {
            workspace_path
                .join(".rupu")
                .is_dir()
                .then_some(workspace_path.clone())
        });
    let workspace_strategy = if repo_ref.is_some() {
        Some("direct_checkout".to_string())
    } else {
        Some("direct_workspace".to_string())
    };

    crate::standalone_run_metadata::StandaloneRunMetadata {
        version: crate::standalone_run_metadata::StandaloneRunMetadata::VERSION,
        run_id: run_id.to_string(),
        session_id: None,
        archived_at: None,
        workspace_path,
        project_root,
        repo_ref,
        issue_ref: None,
        backend_id: "local_checkout".into(),
        worker_id: None,
        trigger_source: "run_cli".into(),
        target: None,
        workspace_strategy,
    }
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
    use std::process::Command;

    fn init_git_checkout(path: &Path, origin_url: &str) {
        let status = Command::new("git")
            .arg("init")
            .arg("-b")
            .arg("main")
            .arg(path)
            .status()
            .unwrap();
        assert!(status.success());
        let status = Command::new("git")
            .arg("-C")
            .arg(path)
            .args(["remote", "add", "origin", origin_url])
            .status()
            .unwrap();
        assert!(status.success());
    }

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
            active_step_id: None,
            active_step_kind: None,
            active_step_agent: None,
            active_step_transcript_path: None,
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
    fn dataset_loads_standalone_run_usage_without_sidecar() {
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
        assert!(dataset.facts[0].repo_ref.is_none());
        assert!(dataset.facts[0].worker_id.is_none());
        assert_eq!(dataset.runs.len(), 1);
    }

    #[test]
    fn dataset_loads_standalone_run_usage_with_sidecar_metadata() {
        let temp = tempfile::tempdir().unwrap();
        let global = temp.path().join(".rupu");
        let transcripts = global.join("transcripts");
        std::fs::create_dir_all(&transcripts).unwrap();
        let started_at = Utc::now();
        write_usage_transcript(
            &transcripts,
            "run_standalone_02",
            "reviewer",
            "anthropic",
            "claude-sonnet-4-6",
            started_at,
            14,
            6,
        );
        let metadata_path = crate::standalone_run_metadata::metadata_path_for_run(
            &transcripts,
            "run_standalone_02",
        );
        crate::standalone_run_metadata::write_metadata(
            &metadata_path,
            &crate::standalone_run_metadata::StandaloneRunMetadata {
                version: crate::standalone_run_metadata::StandaloneRunMetadata::VERSION,
                run_id: "run_standalone_02".into(),
                session_id: None,
                archived_at: None,
                workspace_path: PathBuf::from("/tmp/repo"),
                project_root: Some(PathBuf::from("/tmp/project")),
                repo_ref: Some("github:Section9Labs/rupu".into()),
                issue_ref: Some("github:Section9Labs/rupu/issues/42".into()),
                backend_id: "local_checkout".into(),
                worker_id: Some("worker_local_cli".into()),
                trigger_source: "run_cli".into(),
                target: Some("github:Section9Labs/rupu/issues/42".into()),
                workspace_strategy: Some("direct_checkout".into()),
            },
        )
        .unwrap();

        let dataset = UsageDataset::load(&global, None, TimeWindow::default()).unwrap();
        assert_eq!(dataset.runs.len(), 1);
        assert_eq!(dataset.facts.len(), 1);
        assert_eq!(
            dataset.facts[0].repo_ref.as_deref(),
            Some("github:Section9Labs/rupu")
        );
        assert_eq!(
            dataset.facts[0].issue_ref.as_deref(),
            Some("github:Section9Labs/rupu/issues/42")
        );
        assert_eq!(
            dataset.facts[0].worker_id.as_deref(),
            Some("worker_local_cli")
        );
        assert_eq!(
            dataset.facts[0].backend_id.as_deref(),
            Some("local_checkout")
        );
        assert_eq!(dataset.facts[0].trigger_source.as_deref(), Some("run_cli"));
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

    #[test]
    fn backfill_standalone_metadata_uses_workspace_record() {
        let temp = tempfile::tempdir().unwrap();
        let global = temp.path().join(".rupu");
        let transcripts = global.join("transcripts");
        let workspace_root = temp.path().join("project");
        std::fs::create_dir_all(&transcripts).unwrap();
        std::fs::create_dir_all(&workspace_root).unwrap();
        init_git_checkout(&workspace_root, "git@github.com:Section9Labs/rupu.git");

        let store = rupu_workspace::WorkspaceStore {
            root: global.join("workspaces"),
        };
        let workspace = rupu_workspace::upsert(&store, &workspace_root).unwrap();
        let started_at = Utc::now();
        write_usage_transcript(
            &transcripts,
            "run_backfill_01",
            "reviewer",
            "anthropic",
            "claude-sonnet-4-6",
            started_at,
            10,
            5,
        );

        let transcript_path = transcripts.join("run_backfill_01.jsonl");
        let mut events = JsonlReader::iter(&transcript_path)
            .unwrap()
            .collect::<Vec<_>>();
        let run_start = Event::RunStart {
            run_id: "run_backfill_01".into(),
            workspace_id: workspace.id.clone(),
            agent: "reviewer".into(),
            provider: "anthropic".into(),
            model: "claude-sonnet-4-6".into(),
            started_at,
            mode: rupu_transcript::RunMode::Bypass,
        };
        events[0] = Ok(run_start);
        let mut writer = JsonlWriter::create(&transcript_path).unwrap();
        for event in events {
            writer.write(&event.unwrap()).unwrap();
        }
        writer.flush().unwrap();

        let stats = backfill_standalone_metadata(&global, None, false).unwrap();
        assert_eq!(stats.scanned, 1);
        assert_eq!(stats.backfilled, 1);
        let metadata_path =
            crate::standalone_run_metadata::metadata_path_for_run(&transcripts, "run_backfill_01");
        let metadata = crate::standalone_run_metadata::read_metadata(&metadata_path).unwrap();
        assert_eq!(
            metadata.repo_ref.as_deref(),
            Some("github:Section9Labs/rupu")
        );
        assert_eq!(metadata.backend_id, "local_checkout");
        assert_eq!(metadata.trigger_source, "run_cli");
        assert_eq!(
            metadata.workspace_strategy.as_deref(),
            Some("direct_checkout")
        );
        assert_eq!(metadata.worker_id, None);
    }

    #[test]
    fn backfill_skips_workflow_referenced_transcripts() {
        let temp = tempfile::tempdir().unwrap();
        let global = temp.path().join(".rupu");
        let transcripts = global.join("transcripts");
        let runs_root = global.join("runs");
        std::fs::create_dir_all(&transcripts).unwrap();
        std::fs::create_dir_all(&runs_root).unwrap();

        let started_at = Utc::now();
        let transcript_path = write_usage_transcript(
            &transcripts,
            "step_run_backfill",
            "implementer",
            "openai",
            "gpt-5",
            started_at,
            12,
            4,
        );

        let store = RunStore::new(runs_root);
        store
            .write_run_envelope(
                "run_workflow_backfill",
                &sample_envelope("run_workflow_backfill"),
            )
            .unwrap();
        store
            .create(
                sample_run_record("run_workflow_backfill", started_at, &transcripts),
                "name: phase-delivery-cycle\nsteps: []\n",
            )
            .unwrap();
        store
            .append_step_result(
                "run_workflow_backfill",
                &sample_step_result(&transcript_path),
            )
            .unwrap();

        let stats = backfill_standalone_metadata(&global, None, false).unwrap();
        assert_eq!(stats.scanned, 1);
        assert_eq!(stats.referenced_workflow_transcripts, 1);
        assert_eq!(stats.backfilled, 0);
        let metadata_path = crate::standalone_run_metadata::metadata_path_for_run(
            &transcripts,
            "step_run_backfill",
        );
        assert!(!metadata_path.exists());
    }
}

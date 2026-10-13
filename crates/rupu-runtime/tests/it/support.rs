//! Fixtures for the run-assembly tests: a throwaway `RUPU_HOME` with agents,
//! an assembler over it, and the `RUPU_MOCK_PROVIDER_SCRIPT` seam (callers are
//! `#[serial]`, see `main.rs`).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rupu_runtime::assembly::{AssemblyContext, RunAssembler, WorkspaceBinding};
use rupu_tools::RunIdentity;

/// A one-turn answer.
pub const DONE: &str = r#"[{ "AssistantText": { "text": "child done", "stop": "end_turn" } }]"#;

/// Sets `RUPU_MOCK_PROVIDER_SCRIPT` for its life (cleared on drop, panics
/// included).
pub struct MockScript;

impl MockScript {
    pub fn set(script: &str) -> Self {
        std::env::set_var("RUPU_MOCK_PROVIDER_SCRIPT", script);
        MockScript
    }
}

impl Drop for MockScript {
    fn drop(&mut self) {
        std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
    }
}

pub struct Home {
    pub dir: tempfile::TempDir,
}

impl Home {
    pub fn new() -> Self {
        let home = Self {
            dir: tempfile::tempdir().unwrap(),
        };
        std::fs::create_dir_all(home.global().join("agents")).unwrap();
        std::fs::create_dir_all(home.workspace()).unwrap();
        std::fs::create_dir_all(home.runs()).unwrap();
        home
    }

    pub fn root(&self) -> &Path {
        self.dir.path()
    }

    pub fn global(&self) -> PathBuf {
        self.dir.path().join("global")
    }

    pub fn workspace(&self) -> PathBuf {
        self.dir.path().join("workspace")
    }

    pub fn runs(&self) -> PathBuf {
        self.global().join("runs")
    }

    /// Write `agents/<name>.md` with `frontmatter` (YAML lines) and a prompt.
    pub fn agent(&self, name: &str, frontmatter: &str) {
        std::fs::write(
            self.global().join("agents").join(format!("{name}.md")),
            format!("---\nname: {name}\n{frontmatter}\n---\nyou are {name}.\n"),
        )
        .unwrap();
    }

    /// Install `<global>/skills/<name>` with a reference and a script.
    pub fn skill(&self, name: &str, description: &str, body: &str) -> PathBuf {
        let d = self.global().join("skills").join(name);
        std::fs::create_dir_all(d.join("references")).unwrap();
        std::fs::create_dir_all(d.join("scripts")).unwrap();
        std::fs::write(
            d.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {description}\n---\n{body}\n"),
        )
        .unwrap();
        std::fs::write(d.join("references/REF.md"), "reference text\n").unwrap();
        std::fs::write(
            d.join("scripts/hello.sh"),
            "#!/bin/sh\necho hello-from-skill\n",
        )
        .unwrap();
        d
    }

    pub fn binding(&self) -> WorkspaceBinding {
        WorkspaceBinding {
            id: "ws_test".into(),
            path: self.workspace(),
        }
    }

    pub fn context(&self, config: rupu_config::Config) -> AssemblyContext {
        AssemblyContext {
            global: self.global(),
            project_root: None,
            config,
            customer: None,
            resolver: Arc::new(rupu_auth::KeychainResolver::new()),
            scm: Some(Arc::new(rupu_scm::Registry::default())),
            findings: rupu_coverage::FindingWriteOptions::default(),
            net_capture: None,
            catalog: Some(Arc::new(EmptyCatalog)),
        }
    }

    pub fn assembler(&self, config: rupu_config::Config) -> Arc<RunAssembler> {
        Arc::new(RunAssembler::new(self.context(config)))
    }
}

/// A config whose default provider is the built-in `anthropic`.
pub fn config() -> rupu_config::Config {
    rupu_config::Config {
        default_provider: Some("anthropic".into()),
        default_model: Some("claude-sonnet-4-6".into()),
        ..Default::default()
    }
}

pub fn events(path: &Path) -> Vec<rupu_transcript::Event> {
    rupu_transcript::JsonlReader::iter(path)
        .unwrap()
        .filter_map(Result::ok)
        .collect()
}

/// A catalog with nothing in it: what a site's `FsCatalog` is to the
/// assembler (a port), without the orchestrator.
pub struct EmptyCatalog;

impl rupu_tools::CatalogPort for EmptyCatalog {
    fn agents(&self) -> Result<Vec<rupu_tools::ports::AgentEntry>, rupu_tools::PortError> {
        Ok(Vec::new())
    }
    fn workflows(&self) -> Vec<rupu_tools::ports::WorkflowEntry> {
        Vec::new()
    }
}

/// A flow's status port with no goals, no coverage target and no budget:
/// what an agentiflow hands its lead, for tests that only need the service.
pub struct NoStatus;

impl rupu_tools::RunStatus for NoStatus {
    fn goals(&self) -> Result<rupu_tools::ports::GoalReport, rupu_tools::PortError> {
        Ok(Default::default())
    }
    fn coverage(&self) -> Result<rupu_tools::ports::GoalCoverageReport, rupu_tools::PortError> {
        Ok(rupu_tools::ports::GoalCoverageReport::NoTarget)
    }
    fn budget(&self) -> Result<rupu_tools::ports::BudgetReport, rupu_tools::PortError> {
        Ok(rupu_tools::ports::BudgetReport::NoBudget)
    }
}

/// A launcher that never launches: the assembly tests only check it is
/// offered.
pub struct NoLaunch;

#[async_trait::async_trait]
impl rupu_tools::Launcher for NoLaunch {
    async fn check(
        &self,
        _req: &rupu_tools::ChildRequest,
        _from: &rupu_tools::ToolContext,
    ) -> Result<(), rupu_tools::LaunchError> {
        unreachable!("never launches")
    }
    async fn launch(
        &self,
        _req: rupu_tools::ChildRequest,
        _from: &rupu_tools::ToolContext,
    ) -> Result<rupu_tools::ChildHandle, rupu_tools::LaunchError> {
        unreachable!("never launches")
    }
    async fn join(
        &self,
        _from: &RunIdentity,
        _which: rupu_tools::JoinSet,
        _timeout: std::time::Duration,
        _until: rupu_tools::Until,
    ) -> Vec<rupu_tools::ChildStatus> {
        Vec::new()
    }
    async fn finish(&self, _from: &RunIdentity, _how: rupu_tools::ParentEnd) {}
    fn children(&self, _from: &RunIdentity) -> Vec<rupu_tools::ChildStatus> {
        Vec::new()
    }
}

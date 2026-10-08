//! Fixtures for the run-assembly tests: a throwaway `RUPU_HOME` with agents,
//! an assembler over it, a sub-run store and a recorder for the dispatcher's
//! live events, and the `RUPU_MOCK_PROVIDER_SCRIPT` seam (callers are
//! `#[serial]`, see `main.rs`).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rupu_runtime::assembly::{AssemblyContext, RunAssembler, WorkspaceBinding};
use rupu_runtime::dispatch::{DispatchDone, DispatchEvents, DispatchedChild, SubRunStore};
use rupu_tools::{PermissionMode, RunIdentity, SpawnPermission};

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

/// `<runs>/<parent>/sub/<sub_id>/transcript.jsonl`, the run store's layout.
pub struct DirSubRuns(pub PathBuf);

impl SubRunStore for DirSubRuns {
    fn create_sub_run(
        &self,
        parent_run_id: &str,
        _agent: &str,
    ) -> std::io::Result<(String, PathBuf)> {
        let id = format!("sub_{}", ulid::Ulid::new());
        let dir = self.0.join(parent_run_id).join("sub").join(&id);
        std::fs::create_dir_all(&dir)?;
        let transcript = dir.join("transcript.jsonl");
        std::fs::File::create(&transcript)?;
        Ok((id, transcript))
    }
}

#[derive(Debug, Clone)]
pub enum Seen {
    Started(String, DispatchedChild),
    Completed(String, DispatchDone),
}

#[derive(Default)]
pub struct Recorder(pub Mutex<Vec<Seen>>);

impl DispatchEvents for Recorder {
    fn started(&self, parent_run_id: &str, child: &DispatchedChild) {
        self.0
            .lock()
            .unwrap()
            .push(Seen::Started(parent_run_id.into(), child.clone()));
    }
    fn completed(&self, parent_run_id: &str, done: &DispatchDone) {
        self.0
            .lock()
            .unwrap()
            .push(Seen::Completed(parent_run_id.into(), done.clone()));
    }
}

/// The dispatching parent: a top-level `rupu run` named `parent`.
pub fn parent(codename: Option<&str>) -> RunIdentity {
    RunIdentity {
        run_id: "parent_run_1".into(),
        codename: codename.map(str::to_string),
        agent: "parent".into(),
        provider: "anthropic".into(),
        model: "claude-sonnet-4-6".into(),
        ..Default::default()
    }
}

pub fn bypass() -> SpawnPermission {
    SpawnPermission {
        ceiling: PermissionMode::Bypass,
        prompter: None,
    }
}

pub fn events(path: &Path) -> Vec<rupu_transcript::Event> {
    rupu_transcript::JsonlReader::iter(path)
        .unwrap()
        .filter_map(Result::ok)
        .collect()
}

/// `(provider, model)` of a transcript's `RunStart`.
pub fn run_start(path: &Path) -> (String, String) {
    events(path)
        .into_iter()
        .find_map(|e| match e {
            rupu_transcript::Event::RunStart {
                provider, model, ..
            } => Some((provider, model)),
            _ => None,
        })
        .expect("a RunStart")
}

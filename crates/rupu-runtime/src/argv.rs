//! The one typed command line for a `rupu` child process.
//!
//! Every place that starts a `rupu` child (the CP's local launchers, the SSH
//! connector, the node executor, agentiflow units and coordinators, the
//! session worker, `cp serve`'s resume worker) builds a [`RunArgv`] and
//! renders it here, so prompt quoting, flag spelling, codename passing and
//! run-id honouring cannot differ by launcher. `rupu-cli`'s `argv_roundtrip`
//! test parses every rendering with the real clap `Cli` and converts it back,
//! so this file and the CLI cannot drift apart.
//!
//! Spec: `docs/superpowers/specs/2026-10-07-rupu-tool-and-launch-architecture/W6-process-spawn.md`.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;

use rupu_coverage::FindingProfile;
use rupu_tools::PermissionMode;

/// Host feature: this build's `rupu run` takes the hidden `--codename` flag.
/// An SSH coordinator passes the codename as `RUPU_CODENAME` to a remote
/// whose `rupu __features` does not list it ([`FeatureSet::shell_peer`]).
pub const FEATURE_CODENAME_FLAG: &str = "run.codename_flag";

/// Peer feature: a node (tunnel `Hello.capabilities`, bucket worker marker)
/// or HTTP host (`/api/host/info` `features`) that builds its own `rupu run`
/// command line puts a supplied codename on it as `--codename`
/// ([`FeatureSet::spec_peer`]).
pub const FEATURE_SPEC_CODENAME: &str = "run.codename";

/// The legacy way a codename reached a placed `rupu run`. Still read for one
/// release so in-flight detached runs keep their names, and still how an SSH
/// remote predating [`FEATURE_CODENAME_FLAG`] gets one.
pub const CODENAME_ENV: &str = "RUPU_CODENAME";

/// One `rupu` child invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunArgv {
    /// `rupu run <agent> …`
    Agent(AgentRun),
    /// `rupu workflow run …`
    Workflow(WorkflowRun),
    /// `rupu workflow approve | reject | resume …`
    WorkflowControl(WorkflowControl),
    /// `rupu session start …`
    SessionStart(SessionStart),
    /// `rupu session send …`
    SessionSend(SessionSend),
    /// `rupu session _worker …` — the warm session worker.
    SessionWorker(SessionWorker),
    /// `rupu agentiflow run --run-id <id> -- <definition>` — the detached
    /// child of `agentiflow run --detach`.
    Agentiflow(AgentiflowRun),
}

/// `rupu run <agent> …`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRun {
    pub agent: String,
    /// A run target (`github:owner/repo#42`, …), positional.
    pub target: Option<String>,
    pub run_id: String,
    /// The coordinator-minted codename (`--codename`, hidden).
    pub codename: Option<String>,
    /// Deliver `codename` as [`CODENAME_ENV`] instead of `--codename`: set by
    /// [`RunArgv::for_peer`] for an SSH remote predating the flag.
    pub codename_env: bool,
    pub mode: Option<PermissionMode>,
    pub prompt: Option<String>,
    pub findings_profile: Option<FindingProfile>,
    pub engagement_profiles: Vec<String>,
    /// `--tmp`: clone a repo/PR target into an auto-deleted tmpdir. The CP and
    /// placed launches set it whenever there is a target.
    pub tmp_clone: bool,
    /// Join an agentiflow's board as a pool unit.
    pub flow: Option<FlowAttach>,
}

impl AgentRun {
    /// A bare `rupu run <agent> --run-id <id>`; set the rest by field.
    pub fn new(agent: impl Into<String>, run_id: impl Into<String>) -> Self {
        Self {
            agent: agent.into(),
            target: None,
            run_id: run_id.into(),
            codename: None,
            codename_env: false,
            mode: None,
            prompt: None,
            findings_profile: None,
            engagement_profiles: Vec::new(),
            tmp_clone: false,
            flow: None,
        }
    }
}

/// `rupu workflow run …`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowRun {
    pub workflow: WorkflowRef,
    /// Positional run target. clap refuses one together with
    /// [`WorkflowRef::File`].
    pub target: Option<String>,
    pub run_id: String,
    pub mode: Option<PermissionMode>,
    /// `--input k=v`, rendered in key order.
    pub inputs: BTreeMap<String, String>,
    /// `--plain`: the line printer instead of the live view. Every detached
    /// launch sets it.
    pub plain: bool,
    pub engagement_profiles: Vec<String>,
    pub flow: Option<FlowAttach>,
}

impl WorkflowRun {
    /// A bare `rupu workflow run <workflow> --run-id <id>`; set the rest by
    /// field.
    pub fn new(workflow: WorkflowRef, run_id: impl Into<String>) -> Self {
        Self {
            workflow,
            target: None,
            run_id: run_id.into(),
            mode: None,
            inputs: BTreeMap::new(),
            plain: false,
            engagement_profiles: Vec::new(),
            flow: None,
        }
    }
}

/// Which workflow a [`WorkflowRun`] runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkflowRef {
    /// A catalog name (positional).
    Name(String),
    /// A workflow file (`--file <path>`), not added to the catalog.
    File(PathBuf),
}

/// `--fleet-run-dir <dir> --fleet-participant <id>`: a unit joining an
/// agentiflow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlowAttach {
    pub run_dir: PathBuf,
    pub participant: String,
}

/// `rupu workflow approve | reject | resume <run_id> …`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkflowControl {
    Approve {
        run_id: String,
        mode: Option<PermissionMode>,
        /// The parked gate to approve (`--gate`).
        gate: Option<String>,
        /// The recorded approver (`--approver`, hidden).
        approver: Option<String>,
    },
    Reject {
        run_id: String,
        reason: Option<String>,
        gate: Option<String>,
    },
    Resume {
        run_id: String,
        mode: Option<PermissionMode>,
        /// `--if-unfinished` (hidden): refuse a run that finished since the
        /// resume was requested.
        if_unfinished: bool,
    },
}

/// `rupu session start <agent> …`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionStart {
    pub agent: String,
    pub target: Option<String>,
    pub mode: Option<PermissionMode>,
    pub prompt: Option<String>,
    /// `--into <dir>`: where a repo target is cloned.
    pub into: Option<PathBuf>,
    /// `--detach`: start the first turn without attaching.
    pub detach: bool,
}

/// `rupu session send <session_id> <prompt> …`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSend {
    pub session_id: String,
    pub prompt: String,
    pub detach: bool,
}

/// `rupu session _worker --session-id <id>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionWorker {
    pub session_id: String,
}

/// `rupu agentiflow run --run-id <id> -- <definition>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentiflowRun {
    pub definition: String,
    pub run_id: String,
}

/// What a peer's `rupu` can take, for [`RunArgv::for_peer`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeatureSet {
    codename: CodenameReach,
}

/// How a codename reaches a peer's run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CodenameReach {
    /// As `--codename`.
    Flag,
    /// As [`CODENAME_ENV`]: the peer predates the flag, but we set its
    /// environment.
    Env,
    /// Not at all: the peer builds its own command line and has not
    /// advertised taking a codename.
    None,
}

impl FeatureSet {
    /// This build: every flag [`RunArgv::to_args`] renders.
    pub fn current() -> Self {
        Self {
            codename: CodenameReach::Flag,
        }
    }

    /// A peer that runs the command line we render (SSH), whose
    /// `rupu __features` listed `features`. We set its environment, so a
    /// codename the flag can't carry still reaches it as [`CODENAME_ENV`].
    pub fn shell_peer<S: AsRef<str>>(features: impl IntoIterator<Item = S>) -> Self {
        let flag = features
            .into_iter()
            .any(|f| f.as_ref() == FEATURE_CODENAME_FLAG);
        Self {
            codename: if flag {
                CodenameReach::Flag
            } else {
                CodenameReach::Env
            },
        }
    }

    /// A peer that builds its own command line from a spec (a tunnel or
    /// bucket node, an HTTP host) and advertised `features`. Only what it
    /// advertised reaches the run.
    pub fn spec_peer<S: AsRef<str>>(features: impl IntoIterator<Item = S>) -> Self {
        let codename = features
            .into_iter()
            .any(|f| f.as_ref() == FEATURE_SPEC_CODENAME);
        Self {
            codename: if codename {
                CodenameReach::Flag
            } else {
                CodenameReach::None
            },
        }
    }
}

/// A flag a peer could not take, and what [`RunArgv::for_peer`] did instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Downgrade {
    /// The codename travels as [`CODENAME_ENV`] instead of `--codename`.
    CodenameViaEnv { codename: String },
    /// The codename was dropped: the peer's run picks its own. Cosmetic —
    /// the coordinator's records keep the name it minted.
    CodenameDropped { codename: String },
}

impl std::fmt::Display for Downgrade {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CodenameViaEnv { codename } => write!(
                f,
                "the peer's rupu predates --codename; codename {codename} passed as {CODENAME_ENV}"
            ),
            Self::CodenameDropped { codename } => write!(
                f,
                "the peer has not advertised taking a codename; its run picks its own instead of {codename}"
            ),
        }
    }
}

impl RunArgv {
    /// The canonical rendering, after the executable: one spelling per flag.
    /// A prompt or reject reason is ONE `--prompt=<p>` / `--reason=<r>`
    /// element, so one starting with `-` can't be read as a flag; engagement
    /// ids are repeated, never comma-joined; inputs are `--input k=v` in key
    /// order.
    pub fn to_args(&self) -> Vec<OsString> {
        let mut a = Args::default();
        match self {
            Self::Agent(r) => {
                a.push("run");
                a.push(&r.agent);
                a.opt_positional(r.target.as_deref());
                a.flag_value("--run-id", &r.run_id);
                if !r.codename_env {
                    a.opt_flag_value("--codename", r.codename.as_deref());
                }
                a.opt_flag_value("--mode", r.mode.map(PermissionMode::as_str));
                a.opt_flag_value(
                    "--findings-profile",
                    r.findings_profile.map(FindingProfile::as_str),
                );
                for id in &r.engagement_profiles {
                    a.flag_value("--engagement-profile", id);
                }
                if let Some(p) = &r.prompt {
                    a.push(format!("--prompt={p}"));
                }
                if r.tmp_clone {
                    a.push("--tmp");
                }
                a.flow(r.flow.as_ref());
            }
            Self::Workflow(r) => {
                a.push("workflow");
                a.push("run");
                match &r.workflow {
                    WorkflowRef::Name(name) => a.push(name),
                    WorkflowRef::File(path) => {
                        a.push("--file");
                        a.push(path.as_os_str());
                    }
                }
                a.opt_positional(r.target.as_deref());
                a.flag_value("--run-id", &r.run_id);
                a.opt_flag_value("--mode", r.mode.map(PermissionMode::as_str));
                if r.plain {
                    a.push("--plain");
                }
                for (k, v) in &r.inputs {
                    a.flag_value("--input", &format!("{k}={v}"));
                }
                for id in &r.engagement_profiles {
                    a.flag_value("--engagement-profile", id);
                }
                a.flow(r.flow.as_ref());
            }
            Self::WorkflowControl(c) => {
                a.push("workflow");
                match c {
                    WorkflowControl::Approve {
                        run_id,
                        mode,
                        gate,
                        approver,
                    } => {
                        a.push("approve");
                        a.push(run_id);
                        a.opt_flag_value("--gate", gate.as_deref());
                        a.opt_flag_value("--approver", approver.as_deref());
                        a.opt_flag_value("--mode", mode.map(PermissionMode::as_str));
                    }
                    WorkflowControl::Reject {
                        run_id,
                        reason,
                        gate,
                    } => {
                        a.push("reject");
                        a.push(run_id);
                        if let Some(r) = reason {
                            a.push(format!("--reason={r}"));
                        }
                        a.opt_flag_value("--gate", gate.as_deref());
                    }
                    WorkflowControl::Resume {
                        run_id,
                        mode,
                        if_unfinished,
                    } => {
                        a.push("resume");
                        a.push(run_id);
                        if *if_unfinished {
                            a.push("--if-unfinished");
                        }
                        a.opt_flag_value("--mode", mode.map(PermissionMode::as_str));
                    }
                }
            }
            Self::SessionStart(s) => {
                a.push("session");
                a.push("start");
                a.push(&s.agent);
                a.opt_positional(s.target.as_deref());
                if s.detach {
                    a.push("--detach");
                }
                a.opt_flag_value("--mode", s.mode.map(PermissionMode::as_str));
                if let Some(p) = &s.prompt {
                    a.push(format!("--prompt={p}"));
                }
                if let Some(dir) = &s.into {
                    a.push("--into");
                    a.push(dir.as_os_str());
                }
            }
            Self::SessionSend(s) => {
                a.push("session");
                a.push("send");
                if s.detach {
                    a.push("--detach");
                }
                // The prompt is positional: `--` keeps one starting with `-`
                // from being read as a flag.
                a.push("--");
                a.push(&s.session_id);
                a.push(&s.prompt);
            }
            Self::SessionWorker(w) => {
                a.push("session");
                a.push("_worker");
                a.flag_value("--session-id", &w.session_id);
            }
            Self::Agentiflow(f) => {
                a.push("agentiflow");
                a.push("run");
                a.flag_value("--run-id", &f.run_id);
                // `--` keeps a definition name from being read as a flag.
                a.push("--");
                a.push(&f.definition);
            }
        }
        a.0
    }

    /// The environment the rendered command needs: [`CODENAME_ENV`] when
    /// [`Self::for_peer`] moved the codename there.
    pub fn env(&self) -> Vec<(OsString, OsString)> {
        match self {
            Self::Agent(AgentRun {
                codename: Some(c),
                codename_env: true,
                ..
            }) => vec![(CODENAME_ENV.into(), c.into())],
            _ => Vec::new(),
        }
    }

    /// `program` and [`Self::to_args`], each POSIX-shell-escaped, for an SSH
    /// command string; prefixed with `env 'K=V'` when [`Self::env`] has any.
    pub fn to_shell(&self, program: &str) -> String {
        let mut words: Vec<String> = Vec::new();
        let env = self.env();
        if !env.is_empty() {
            words.push(shell_escape("env"));
            for (k, v) in env {
                words.push(shell_escape(&format!(
                    "{}={}",
                    k.to_string_lossy(),
                    v.to_string_lossy()
                )));
            }
        }
        words.push(shell_escape(program));
        words.extend(
            self.to_args()
                .iter()
                .map(|a| shell_escape(&a.to_string_lossy())),
        );
        words.join(" ")
    }

    /// The argv to send a peer that may predate a flag: each flag its
    /// `features` don't cover is moved or dropped, and the returned
    /// [`Downgrade`]s say which (the caller logs them — never a silent
    /// drop). Only the codename can be downgraded: every other flag changes
    /// how the run behaves, so a connector refuses a peer that can't take
    /// it instead.
    pub fn for_peer(&self, features: &FeatureSet) -> (RunArgv, Vec<Downgrade>) {
        let mut out = self.clone();
        let mut downgrades = Vec::new();
        if let Self::Agent(r) = &mut out {
            if let Some(c) = r.codename.clone() {
                match features.codename {
                    CodenameReach::Flag => r.codename_env = false,
                    CodenameReach::Env => {
                        r.codename_env = true;
                        downgrades.push(Downgrade::CodenameViaEnv { codename: c });
                    }
                    CodenameReach::None => {
                        r.codename = None;
                        r.codename_env = false;
                        downgrades.push(Downgrade::CodenameDropped { codename: c });
                    }
                }
            }
        }
        (out, downgrades)
    }
}

/// POSIX single-quote escaping: wrap in single quotes, replacing each
/// embedded `'` with `'\''`.
pub fn shell_escape(arg: &str) -> String {
    let mut out = String::with_capacity(arg.len() + 2);
    out.push('\'');
    for ch in arg.chars() {
        if ch == '\'' {
            out.push_str(r"'\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

/// The argv under construction.
#[derive(Default)]
struct Args(Vec<OsString>);

impl Args {
    fn push(&mut self, a: impl Into<OsString>) {
        self.0.push(a.into());
    }

    fn opt_positional(&mut self, a: Option<&str>) {
        if let Some(a) = a {
            self.push(a);
        }
    }

    fn flag_value(&mut self, flag: &str, value: &str) {
        self.push(flag);
        self.push(value);
    }

    fn opt_flag_value(&mut self, flag: &str, value: Option<&str>) {
        if let Some(v) = value {
            self.flag_value(flag, v);
        }
    }

    fn flow(&mut self, flow: Option<&FlowAttach>) {
        if let Some(f) = flow {
            self.push("--fleet-run-dir");
            self.push(f.run_dir.as_os_str());
            self.flag_value("--fleet-participant", &f.participant);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strs(argv: &RunArgv) -> Vec<String> {
        argv.to_args()
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    fn placed_agent() -> RunArgv {
        let mut r = AgentRun::new("recon", "run_01ABC");
        r.codename = Some("cobalt-harbor/heron#412".into());
        r.prompt = Some("- scan\nit's \"all\"".into());
        r.engagement_profiles = vec!["network".into(), "web".into()];
        RunArgv::Agent(r)
    }

    #[test]
    fn an_agent_run_renders_one_spelling_per_flag() {
        let mut r = AgentRun::new("triage", "run_X");
        r.target = Some("github:o/r".into());
        r.mode = Some(PermissionMode::Bypass);
        r.findings_profile = Some(FindingProfile::Summary);
        r.engagement_profiles = vec!["network".into(), "web".into()];
        r.prompt = Some("look at PR".into());
        r.tmp_clone = true;
        r.codename = Some("jade-reef/numbat".into());
        r.flow = Some(FlowAttach {
            run_dir: "/g/agentiflows/af_1".into(),
            participant: "recon#1".into(),
        });
        assert_eq!(
            strs(&RunArgv::Agent(r)),
            [
                "run",
                "triage",
                "github:o/r",
                "--run-id",
                "run_X",
                "--codename",
                "jade-reef/numbat",
                "--mode",
                "bypass",
                "--findings-profile",
                "summary",
                "--engagement-profile",
                "network",
                "--engagement-profile",
                "web",
                "--prompt=look at PR",
                "--tmp",
                "--fleet-run-dir",
                "/g/agentiflows/af_1",
                "--fleet-participant",
                "recon#1",
            ]
        );
    }

    #[test]
    fn a_prompt_is_one_attached_element() {
        let argv = strs(&placed_agent());
        assert!(argv.contains(&"--prompt=- scan\nit's \"all\"".to_string()));
        assert!(!argv.iter().any(|a| a == "--prompt"));
    }

    #[test]
    fn a_workflow_run_renders_inputs_in_key_order() {
        let mut r = WorkflowRun::new(WorkflowRef::Name("audit".into()), "run_X");
        r.target = Some("github:o/r".into());
        r.mode = Some(PermissionMode::Bypass);
        r.plain = true;
        r.inputs.insert("k".into(), "v".into());
        r.inputs.insert("a".into(), "b=c d".into());
        assert_eq!(
            strs(&RunArgv::Workflow(r)),
            [
                "workflow",
                "run",
                "audit",
                "github:o/r",
                "--run-id",
                "run_X",
                "--mode",
                "bypass",
                "--plain",
                "--input",
                "a=b=c d",
                "--input",
                "k=v",
            ]
        );
    }

    #[test]
    fn a_file_workflow_has_no_positional_name() {
        let r = WorkflowRun::new(WorkflowRef::File("/w/gen.yaml".into()), "run_X");
        assert_eq!(
            strs(&RunArgv::Workflow(r)),
            [
                "workflow",
                "run",
                "--file",
                "/w/gen.yaml",
                "--run-id",
                "run_X"
            ]
        );
    }

    #[test]
    fn workflow_controls_render() {
        let approve = RunArgv::WorkflowControl(WorkflowControl::Approve {
            run_id: "run_x".into(),
            mode: Some(PermissionMode::Bypass),
            gate: Some("gate_b".into()),
            approver: Some("web".into()),
        });
        assert_eq!(
            strs(&approve),
            [
                "workflow",
                "approve",
                "run_x",
                "--gate",
                "gate_b",
                "--approver",
                "web",
                "--mode",
                "bypass"
            ]
        );
        let resume = RunArgv::WorkflowControl(WorkflowControl::Resume {
            run_id: "run_x".into(),
            mode: None,
            if_unfinished: true,
        });
        assert_eq!(
            strs(&resume),
            ["workflow", "resume", "run_x", "--if-unfinished"]
        );
        let reject = RunArgv::WorkflowControl(WorkflowControl::Reject {
            run_id: "run_x".into(),
            reason: Some("-not now".into()),
            gate: None,
        });
        assert_eq!(
            strs(&reject),
            ["workflow", "reject", "run_x", "--reason=-not now"]
        );
    }

    #[test]
    fn a_session_send_prompt_follows_the_separator() {
        let send = RunArgv::SessionSend(SessionSend {
            session_id: "ses_1".into(),
            prompt: "--help me".into(),
            detach: true,
        });
        assert_eq!(
            strs(&send),
            ["session", "send", "--detach", "--", "ses_1", "--help me"]
        );
    }

    #[test]
    fn for_peer_keeps_the_flag_for_a_peer_that_has_it() {
        let argv = placed_agent();
        let (same, downgrades) = argv.for_peer(&FeatureSet::current());
        assert_eq!(same, argv);
        assert!(downgrades.is_empty());
        let (same, downgrades) = argv.for_peer(&FeatureSet::shell_peer([FEATURE_CODENAME_FLAG]));
        assert_eq!(same, argv);
        assert!(downgrades.is_empty());
        let (same, downgrades) = argv.for_peer(&FeatureSet::spec_peer([FEATURE_SPEC_CODENAME]));
        assert_eq!(same, argv);
        assert!(downgrades.is_empty());
    }

    #[test]
    fn for_peer_downgrades_an_old_shell_peer_to_the_env_form() {
        let (old, downgrades) = placed_agent().for_peer(&FeatureSet::shell_peer(["x"]));
        assert_eq!(
            downgrades,
            [Downgrade::CodenameViaEnv {
                codename: "cobalt-harbor/heron#412".into()
            }]
        );
        assert!(!strs(&old).iter().any(|a| a == "--codename"));
        assert_eq!(
            old.env(),
            [(
                OsString::from(CODENAME_ENV),
                OsString::from("cobalt-harbor/heron#412")
            )]
        );
        assert!(old
            .to_shell("rupu")
            .starts_with("'env' 'RUPU_CODENAME=cobalt-harbor/heron#412' 'rupu' 'run' 'recon'"));
    }

    #[test]
    fn for_peer_drops_the_codename_for_a_spec_peer_without_it() {
        let (old, downgrades) =
            placed_agent().for_peer(&FeatureSet::spec_peer(Vec::<String>::new()));
        assert_eq!(
            downgrades,
            [Downgrade::CodenameDropped {
                codename: "cobalt-harbor/heron#412".into()
            }]
        );
        let RunArgv::Agent(r) = &old else { panic!() };
        assert_eq!(r.codename, None);
        assert!(old.env().is_empty());
    }

    #[test]
    fn for_peer_has_nothing_to_downgrade_without_a_codename() {
        let argv = RunArgv::Agent(AgentRun::new("a", "run_1"));
        assert!(argv
            .for_peer(&FeatureSet::spec_peer(Vec::<String>::new()))
            .1
            .is_empty());
    }

    #[test]
    fn shell_escape_wraps_and_escapes_quotes() {
        assert_eq!(shell_escape("plain"), "'plain'");
        assert_eq!(shell_escape("it's"), r"'it'\''s'");
        assert_eq!(shell_escape("$HOME"), "'$HOME'");
    }

    /// `to_shell`, run through a real `sh`, reproduces `to_args` element for
    /// element (spec W6 §6 test 2).
    #[cfg(unix)]
    #[test]
    fn argv_shell_safe() {
        let mut nasty = AgentRun::new("recon", "run_01ABC");
        nasty.target = Some("github:o/r#4".into());
        nasty.prompt = Some("- it's $HOME `id` \"q\" \\ \n second line; rm -rf /".into());
        nasty.flow = Some(FlowAttach {
            run_dir: "/tmp/a dir/with 'quotes'".into(),
            participant: "recon#1".into(),
        });
        let mut wf = WorkflowRun::new(WorkflowRef::Name("audit".into()), "run_X");
        wf.inputs.insert("q".into(), "a'b\"c$d".into());
        for argv in [
            RunArgv::Agent(nasty),
            RunArgv::Workflow(wf),
            RunArgv::SessionSend(SessionSend {
                session_id: "ses_1".into(),
                prompt: "-x 'y'".into(),
                detach: true,
            }),
        ] {
            let script = format!("printf '%s\\0' {}", argv.to_shell("rupu"));
            let out = std::process::Command::new("/bin/sh")
                .args(["-c", &script])
                .output()
                .unwrap();
            assert!(out.status.success(), "{script}");
            let mut got: Vec<String> = out
                .stdout
                .split(|b| *b == 0)
                .map(|w| String::from_utf8(w.to_vec()).unwrap())
                .collect();
            got.pop(); // the trailing NUL's empty tail
            let mut want = vec!["rupu".to_string()];
            want.extend(strs(&argv));
            assert_eq!(got, want);
        }
    }
}

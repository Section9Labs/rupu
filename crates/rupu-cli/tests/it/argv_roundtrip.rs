//! W6 §6 test 1: every `RunArgv` a launcher renders parses with the real
//! clap `Cli` back into the same `RunArgv`. `rupu_runtime::argv` and the
//! CLI's flag definitions cannot drift apart: a renamed, dropped or
//! re-typed flag fails here, not on a detached child nobody watches.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;

use clap::Parser as _;
use rupu_cli::cmd::{self, run::RunAction};
use rupu_cli::{Cli, Cmd};
use rupu_coverage::FindingProfile;
use rupu_runtime::argv::{
    AgentRun, AgentiflowRun, FlowAttach, RunArgv, SessionSend, SessionStart, SessionWorker,
    WorkflowControl, WorkflowRef, WorkflowRun,
};
use rupu_tools::PermissionMode;

/// Prompts that break naive renderings: a leading `-`, a bare `--`, newlines,
/// both quote kinds, shell metacharacters, an `=` and trailing whitespace.
const PROMPTS: &[&str] = &[
    "look at PR",
    "- enumerate the hosts",
    "--help",
    "-x",
    "line one\nline \"two\"\n'three'",
    "$HOME `id` ; rm -rf / && echo |",
    "k=v --prompt=again ",
];

const MODES: [Option<PermissionMode>; 4] = [
    None,
    Some(PermissionMode::Ask),
    Some(PermissionMode::Bypass),
    Some(PermissionMode::Readonly),
];

fn flow() -> FlowAttach {
    FlowAttach {
        run_dir: PathBuf::from("/g/agentiflows/af_01X with space"),
        participant: "recon#1".into(),
    }
}

fn agent_runs() -> Vec<RunArgv> {
    let mut out = Vec::new();
    // Every combination of the optional parts, each prompt in turn.
    for mask in 0u32..(1 << 7) {
        let on = |bit: u32| mask & (1 << bit) != 0;
        let mut r = AgentRun::new("recon", "run_01ROUNDTRIP");
        if on(0) {
            r.target = Some("github:o/r#42".into());
            r.tmp_clone = true;
        }
        if on(1) {
            r.codename = Some("cobalt-harbor/heron#412".into());
        }
        r.mode = MODES[(mask as usize >> 2) % MODES.len()];
        if on(4) {
            r.findings_profile = Some(if on(0) {
                FindingProfile::Full
            } else {
                FindingProfile::Summary
            });
        }
        if on(5) {
            r.engagement_profiles = vec!["network".into(), "web".into()];
        }
        if on(6) {
            r.flow = Some(flow());
        }
        r.prompt = match mask as usize % (PROMPTS.len() + 1) {
            0 => None,
            i => Some(PROMPTS[i - 1].to_string()),
        };
        out.push(RunArgv::Agent(r));
    }
    // A clone without a target is the caller's choice too.
    let mut tmp_only = AgentRun::new("recon", "run_01ROUNDTRIP");
    tmp_only.tmp_clone = true;
    out.push(RunArgv::Agent(tmp_only));
    out
}

fn workflow_runs() -> Vec<RunArgv> {
    let mut out = Vec::new();
    for mask in 0u32..(1 << 7) {
        let on = |bit: u32| mask & (1 << bit) != 0;
        let file = on(0);
        let mut r = WorkflowRun::new(
            if file {
                WorkflowRef::File(PathBuf::from("/runs/x/generated/gen abc.yaml"))
            } else {
                WorkflowRef::Name("web-assess".into())
            },
            "run_01ROUNDTRIP",
        );
        // clap refuses `--file` together with a target.
        if on(1) && !file {
            r.target = Some("github:o/r".into());
        }
        r.mode = MODES[(mask as usize >> 2) % MODES.len()];
        r.plain = on(4);
        if on(5) {
            r.inputs = BTreeMap::from([
                ("depth".to_string(), "a=b c".to_string()),
                ("target".to_string(), "-x 'y'".to_string()),
            ]);
            r.engagement_profiles = vec!["web".into()];
        }
        if on(6) {
            r.flow = Some(flow());
        }
        out.push(RunArgv::Workflow(r));
    }
    out
}

fn controls() -> Vec<RunArgv> {
    let mut out = Vec::new();
    for mode in MODES {
        for gate in [None, Some("gate_b".to_string())] {
            for approver in [None, Some("web".to_string())] {
                out.push(RunArgv::WorkflowControl(WorkflowControl::Approve {
                    run_id: "run_x".into(),
                    mode,
                    gate: gate.clone(),
                    approver,
                }));
            }
            for reason in [None, Some("-not now\nsorry".to_string())] {
                out.push(RunArgv::WorkflowControl(WorkflowControl::Reject {
                    run_id: "run_x".into(),
                    reason,
                    gate: gate.clone(),
                }));
            }
        }
        for if_unfinished in [false, true] {
            out.push(RunArgv::WorkflowControl(WorkflowControl::Resume {
                run_id: "run_x".into(),
                mode,
                if_unfinished,
            }));
        }
    }
    out
}

fn sessions() -> Vec<RunArgv> {
    let mut out = Vec::new();
    for mask in 0u32..(1 << 4) {
        let on = |bit: u32| mask & (1 << bit) != 0;
        let target = on(0).then(|| "github:o/r".to_string());
        out.push(RunArgv::SessionStart(SessionStart {
            agent: "triage".into(),
            into: target
                .as_ref()
                .filter(|_| on(1))
                .map(|_| PathBuf::from("/clones/x y")),
            target,
            mode: MODES[mask as usize % MODES.len()],
            prompt: on(2).then(|| PROMPTS[mask as usize % PROMPTS.len()].to_string()),
            detach: on(3),
        }));
    }
    for prompt in PROMPTS {
        for detach in [false, true] {
            out.push(RunArgv::SessionSend(SessionSend {
                session_id: "ses_01X".into(),
                prompt: prompt.to_string(),
                detach,
            }));
        }
    }
    out.push(RunArgv::SessionWorker(SessionWorker {
        session_id: "ses_01X".into(),
    }));
    out
}

fn agentiflows() -> Vec<RunArgv> {
    ["acme", "-x"]
        .into_iter()
        .map(|def| {
            RunArgv::Agentiflow(AgentiflowRun {
                definition: def.into(),
                run_id: "af_01X".into(),
            })
        })
        .collect()
}

fn mode(word: Option<String>) -> Option<PermissionMode> {
    word.map(|w| PermissionMode::parse(&w).expect("a mode word clap accepted"))
}

fn flow_of(run_dir: Option<PathBuf>, participant: Option<String>) -> Option<FlowAttach> {
    match (run_dir, participant) {
        (Some(run_dir), Some(participant)) => Some(FlowAttach {
            run_dir,
            participant,
        }),
        (None, None) => None,
        other => panic!("clap let half a fleet pair through: {other:?}"),
    }
}

/// The `RunArgv` a parsed command line stands for: the inverse of
/// `RunArgv::to_args`.
fn back(cli: Cli) -> RunArgv {
    match cli.command {
        Cmd::Run { argv } => {
            let RunAction::Launch(argv) = cmd::run::classify(argv).unwrap() else {
                panic!("an agent run classified as a run control action");
            };
            let a = cmd::run::parse_launch_args(argv).unwrap();
            assert_eq!(a.prompt, None, "the prompt is never positional");
            RunArgv::Agent(AgentRun {
                agent: a.agent,
                target: a.target,
                run_id: a.run_id.expect("every agent run carries its id"),
                codename: a.codename.map(|c| c.to_string()),
                codename_env: false,
                mode: mode(a.mode),
                prompt: a.prompt_flag,
                findings_profile: a.findings_profile,
                engagement_profiles: a.engagement_profiles,
                tmp_clone: a.tmp,
                flow: flow_of(a.fleet_run_dir, a.fleet_participant),
            })
        }
        Cmd::Workflow { action } => match action {
            cmd::workflow::Action::Run {
                name,
                target,
                input,
                mode: m,
                plain,
                run_id,
                engagement_profiles,
                fleet_run_dir,
                fleet_participant,
                file,
                ..
            } => RunArgv::Workflow(WorkflowRun {
                workflow: match (name, file) {
                    (Some(name), None) => WorkflowRef::Name(name),
                    (None, Some(file)) => WorkflowRef::File(file),
                    other => panic!("clap let a name AND a file through: {other:?}"),
                },
                target,
                run_id: run_id.expect("every workflow run carries its id"),
                mode: mode(m),
                inputs: input.into_iter().collect(),
                plain,
                engagement_profiles,
                flow: flow_of(fleet_run_dir, fleet_participant),
            }),
            cmd::workflow::Action::Approve {
                run_id,
                mode: m,
                gate,
                approver,
            } => RunArgv::WorkflowControl(WorkflowControl::Approve {
                run_id,
                mode: mode(m),
                gate,
                approver,
            }),
            cmd::workflow::Action::Reject {
                run_id,
                reason,
                gate,
            } => RunArgv::WorkflowControl(WorkflowControl::Reject {
                run_id,
                reason,
                gate,
            }),
            cmd::workflow::Action::Resume {
                run_id,
                mode: m,
                plain,
                if_unfinished,
                restart_interrupted,
            } => {
                assert!(!plain && !restart_interrupted, "never rendered");
                RunArgv::WorkflowControl(WorkflowControl::Resume {
                    run_id,
                    mode: mode(m),
                    if_unfinished,
                })
            }
            other => panic!("not a rendered workflow command: {other:?}"),
        },
        Cmd::Session { action } => match action {
            cmd::session::Action::Start(s) => {
                assert_eq!(s.prompt, None, "the prompt is never positional");
                RunArgv::SessionStart(SessionStart {
                    agent: s.agent,
                    target: s.target,
                    mode: mode(s.mode),
                    prompt: s.prompt_flag,
                    into: s.into,
                    detach: s.detach,
                })
            }
            cmd::session::Action::Send(s) => RunArgv::SessionSend(SessionSend {
                session_id: s.session_id,
                prompt: s.prompt,
                detach: s.detach,
            }),
            cmd::session::Action::RunWorker(w) => RunArgv::SessionWorker(SessionWorker {
                session_id: w.session_id,
            }),
            other => panic!("not a rendered session command: {other:?}"),
        },
        Cmd::Agentiflow {
            action:
                cmd::agentiflow::Action::Run {
                    def,
                    detach,
                    run_id,
                },
        } => {
            assert!(!detach, "the detached child must not detach again");
            RunArgv::Agentiflow(AgentiflowRun {
                definition: def,
                run_id: run_id.expect("the detached child carries its id"),
            })
        }
        other => panic!("not a rendered command: {other:?}"),
    }
}

#[test]
fn argv_roundtrip() {
    let all: Vec<RunArgv> = [
        agent_runs(),
        workflow_runs(),
        controls(),
        sessions(),
        agentiflows(),
    ]
    .concat();
    assert!(all.len() > 300, "the generated set shrank: {}", all.len());
    for argv in all {
        let full: Vec<OsString> = std::iter::once(OsString::from("rupu"))
            .chain(argv.to_args())
            .collect();
        let cli =
            Cli::try_parse_from(&full).unwrap_or_else(|e| panic!("clap rejected {full:?}:\n{e}"));
        assert_eq!(back(cli), argv, "{full:?}");
    }
}

/// The legacy env form an old SSH remote gets keeps every other flag: only
/// `--codename` moves (to `RUPU_CODENAME`, which `rupu run` still reads).
#[test]
fn the_env_form_drops_only_the_codename_flag() {
    use rupu_runtime::argv::FeatureSet;
    for argv in agent_runs() {
        let (legacy, _) = argv.for_peer(&FeatureSet::shell_peer(Vec::<String>::new()));
        let full: Vec<OsString> = std::iter::once(OsString::from("rupu"))
            .chain(legacy.to_args())
            .collect();
        let RunArgv::Agent(mut want) = argv else {
            unreachable!()
        };
        let RunArgv::Agent(got) = back(Cli::try_parse_from(&full).unwrap()) else {
            unreachable!()
        };
        assert_eq!(got.codename, None, "{full:?}");
        want.codename = None;
        assert_eq!(got, want, "{full:?}");
    }
}

use super::*;
use crate::assembly::{LaunchServices, Overrides, ParentRun, WorkspaceBinding};
use rupu_tools::{PermissionMode, RunIdentity};
use serde_json::json;

struct Home {
    _temp: tempfile::TempDir,
    global: PathBuf,
}
impl Home {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let global = temp.path().canonicalize().unwrap();
        Self {
            _temp: temp,
            global,
        }
    }
    fn session(&self) -> LaunchSpec {
        let dir = self.global.join("sessions/ses_queue");
        fs::create_dir_all(&dir).unwrap();
        let now = chrono::Utc::now();
        let record:crate::sessions::SessionRecord=serde_json::from_value(json!({
                "version":1,"session_id":"ses_queue","agent_name":"rupu-session","provider_name":"anthropic","model":"test-model",
                "agent_system_prompt":"Inspect project work","max_turns":10,"permission_mode":"readonly","no_stream":true,
                "workspace_id":"ws_queue","workspace_path":self.global,"transcripts_dir":dir.join("turns"),
                "created_at":now,"updated_at":now,"status":"running"
            })).expect("fixture must satisfy the production SessionRecord schema");
        fs::write(
            dir.join("session.json"),
            serde_json::to_vec(&record).unwrap(),
        )
        .unwrap();
        LaunchSpec {
            agent: rupu_agent::builtin::session_agent(),
            origin: Origin::SessionTurn {
                session_id: "ses_queue".into(),
                session_dir: dir.clone(),
            },
            run_id: "run_turn".into(),
            transcript_path: dir.join("turns/run_turn.jsonl"),
            codename: None,
            prompt: rupu_agent::UserTurn::new("inspect queued work"),
            workspace: WorkspaceBinding {
                id: "ws_queue".into(),
                path: self.global.clone(),
            },
            mode: PermissionMode::Readonly,
            ceiling: None,
            prompter: None,
            overrides: Overrides::default(),
            stream: Default::default(),
            hooks: Default::default(),
            pause: None,
            services: LaunchServices::default(),
            collectors: Vec::new(),
            link: None,
        }
    }
    fn queue(&self, spec: &LaunchSpec, execution: &str) -> (Arc<dyn WorkQueuePort>, Arc<Bus>) {
        let Origin::SessionTurn { session_dir, .. } = &spec.origin else {
            panic!("session helper")
        };
        let bus = Arc::new(Bus::open(&session_dir.join("messages"), "lead").staying());
        let queue = bind(&self.global, None, spec, bus.clone(), execution.into()).unwrap();
        bus.join(rupu_fleet::Member {
            run_id: spec.run_id.clone(),
            agent: spec.agent.name.clone(),
            work_execution_id: Some(execution.into()),
            kind: "session_turn".into(),
            ..Default::default()
        })
        .unwrap();
        (queue, bus)
    }
}
fn source() -> WorkSource {
    WorkSource {
        turn_index: 0,
        call_sequence: 0,
        tool_call_id: "same_provider_id".into(),
    }
}
fn post(id: &str) -> WorkCommand {
    WorkCommand {
        request_id: id.into(),
        expected_queue_id: None,
        expected_revision: 0,
        operation: WorkOperation::Post {
            title: "Review a real project change".into(),
            instructions: "Inspect the supplied source and report evidence".into(),
        },
    }
}

#[test]
fn exact_owner_and_recording_bind_without_creating_a_queue() {
    let home = Home::new();
    let spec = home.session();
    let (queue, bus) = home.queue(&spec, "exec_one");
    let WorkActor::Agent {
        recording,
        execution_id,
        ..
    } = queue.actor()
    else {
        panic!()
    };
    assert_eq!(execution_id, "exec_one");
    assert_eq!(recording.anchor, WorkRecordingAnchor::Owner);
    assert_eq!(recording.relative_path, "turns/run_turn.jsonl");
    let page = queue
        .list(WorkPageQuery {
            filters: Default::default(),
            limit: 8,
            cursor: None,
        })
        .unwrap();
    assert!(page.items.is_empty());
    assert!(page.snapshot.queue_id.is_none());
    let collector = QueueCollector { queue };
    assert!(collector
        .collect(&TurnContext {
            run_id: spec.run_id.clone(),
            codename: None,
            participant: "lead".into(),
            turn_index: 0
        })
        .is_empty());
    assert!(
        !bus.space().join("board").exists(),
        "observations must not create queue storage"
    );
}

#[test]
fn retained_context_cannot_write_after_execution_end_but_can_replay() {
    let home = Home::new();
    let spec = home.session();
    let (queue, _) = home.queue(&spec, "exec_one");
    let command = post("request_first");
    let first = queue.apply(command.clone(), source()).unwrap();
    queue.finish_execution();
    let replay = queue
        .apply(
            command,
            WorkSource {
                turn_index: 9,
                ..source()
            },
        )
        .unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.receipt, first.receipt);
    let fresh = WorkCommand {
        request_id: "request_second".into(),
        expected_queue_id: Some(first.receipt.queue_id.clone()),
        expected_revision: first.queue_revision,
        operation: WorkOperation::Post {
            title: "Second".into(),
            instructions: "Must not commit".into(),
        },
    };
    assert!(queue.apply(fresh, source()).is_err());
}

#[test]
fn lifecycle_lease_precedes_bus_and_storage_mutation() {
    let home = Home::new();
    let spec = home.session();
    let (queue, bus) = home.queue(&spec, "exec_one");
    let lease = RetentionLease::try_acquire(&home.global).unwrap();
    assert!(matches!(
        queue.apply(post("request_busy"), source()),
        Err(WorkError::Busy)
    ));
    assert!(!bus.space().join("board").exists());
    drop(lease);
    assert!(queue.apply(post("request_busy"), source()).is_ok());
}

#[test]
fn mutable_session_save_is_allowed_but_replaced_owner_record_is_fenced() {
    let home = Home::new();
    let spec = home.session();
    let (queue, _) = home.queue(&spec, "exec_one");
    let path = home.global.join("sessions/ses_queue/session.json");
    let mut record: crate::sessions::SessionRecord =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    record.updated_at = chrono::Utc::now();
    record.total_turns = 11;
    fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
    let first = queue.apply(post("before_replacement"), source()).unwrap();
    record.created_at += chrono::Duration::seconds(1);
    fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
    let next = WorkCommand {
        request_id: "after_replacement".into(),
        expected_queue_id: Some(first.receipt.queue_id),
        expected_revision: first.queue_revision,
        operation: WorkOperation::Post {
            title: "No".into(),
            instructions: "No".into(),
        },
    };
    assert!(queue.apply(next, source()).is_err());
}

#[test]
fn exact_execution_replacement_fences_same_run_and_recording() {
    let home = Home::new();
    let spec = home.session();
    let (old, bus) = home.queue(&spec, "exec_old");
    let command = post("request_old");
    let first = old.apply(command.clone(), source()).unwrap();
    let _new = bind(&home.global, None, &spec, bus.clone(), "exec_new".into()).unwrap();
    bus.join(rupu_fleet::Member {
        run_id: spec.run_id.clone(),
        agent: spec.agent.name.clone(),
        work_execution_id: Some("exec_new".into()),
        kind: "session_turn".into(),
        ..Default::default()
    })
    .unwrap();
    assert!(old.apply(command, source()).unwrap().replayed);
    let claim = WorkCommand {
        request_id: "request_claim".into(),
        expected_queue_id: Some(first.receipt.queue_id.clone()),
        expected_revision: first.current_item.revision,
        operation: WorkOperation::Claim {
            work_id: first.current_item.work_id,
        },
    };
    assert!(old.apply(claim, source()).is_err());
    assert_eq!(
        bus.roster()
            .unwrap()
            .get("lead")
            .unwrap()
            .member
            .work_execution_id
            .as_deref(),
        Some("exec_new")
    );
}

#[test]
fn bus_path_or_continuation_symlink_does_not_establish_owner() {
    let home = Home::new();
    let spec = home.session();
    let foreign = Arc::new(Bus::open(&home.global.join("elsewhere"), "lead"));
    assert!(bind(&home.global, None, &spec, foreign, "exec_one".into()).is_err());
    let Origin::SessionTurn { session_dir, .. } = &spec.origin else {
        panic!()
    };
    fs::create_dir(home.global.join("old_messages")).unwrap();
    std::os::unix::fs::symlink(
        home.global.join("old_messages"),
        session_dir.join("messages"),
    )
    .unwrap();
    let alias = Arc::new(Bus::open(&session_dir.join("messages"), "lead"));
    assert!(bind(&home.global, None, &spec, alias, "exec_one".into()).is_err());
}

#[test]
fn configured_root_alias_is_supported_without_trusting_message_aliases() {
    let home = Home::new();
    let mut spec = home.session();
    let link_parent = tempfile::tempdir().unwrap();
    let alias = link_parent.path().join("configured-rupu-home");
    std::os::unix::fs::symlink(&home.global, &alias).unwrap();
    let directory = alias.join("sessions/ses_queue");
    spec.origin = Origin::SessionTurn {
        session_id: "ses_queue".into(),
        session_dir: directory.clone(),
    };
    spec.transcript_path = directory.join("turns/run_turn.jsonl");
    let bus = Arc::new(Bus::open(&directory.join("messages"), "lead"));
    let queue = bind(&alias, None, &spec, bus.clone(), "exec_alias".into()).unwrap();
    bus.join(rupu_fleet::Member {
        run_id: spec.run_id.clone(),
        agent: spec.agent.name.clone(),
        work_execution_id: Some("exec_alias".into()),
        kind: "session_turn".into(),
        ..Default::default()
    })
    .unwrap();
    assert_eq!(queue.binding().global, home.global);
    assert!(queue
        .apply(post("through_configured_alias"), source())
        .is_ok());
}

#[test]
fn archived_owner_is_not_recreated_by_a_retained_context() {
    fn contents(root: &Path) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
        fn scan(root: &Path, path: &Path, out: &mut std::collections::BTreeMap<PathBuf, Vec<u8>>) {
            for entry in fs::read_dir(path).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    scan(root, &path, out);
                } else {
                    out.insert(
                        path.strip_prefix(root).unwrap().into(),
                        fs::read(path).unwrap(),
                    );
                }
            }
        }
        let mut out = Default::default();
        scan(root, root, &mut out);
        out
    }
    let home = Home::new();
    let spec = home.session();
    let (queue, _) = home.queue(&spec, "exec_archive");
    let first = queue.apply(post("before_archive"), source()).unwrap();
    let active = home.global.join("sessions/ses_queue");
    let archive = home.global.join("sessions-archive/ses_queue");
    fs::create_dir_all(archive.parent().unwrap()).unwrap();
    {
        let _lease = RetentionLease::for_legacy(&home.global).unwrap();
        fs::rename(&active, &archive).unwrap();
    }
    let before = contents(&archive);
    let request = WorkCommand {
        request_id: "after_archive".into(),
        expected_queue_id: Some(first.receipt.queue_id),
        expected_revision: first.queue_revision,
        operation: WorkOperation::Post {
            title: "Must not recreate".into(),
            instructions: "Must not commit".into(),
        },
    };
    assert!(queue.apply(request, source()).is_err());
    assert!(
        !active.exists(),
        "guard must run before Bus can create a message space"
    );
    assert_eq!(contents(&archive), before);
}

#[test]
fn child_requires_typed_parent_owner_and_exact_parent_recording_path() {
    let home = Home::new();
    let mut spec = home.session();
    let (parent, bus) = home.queue(&spec, "exec_parent");
    spec.origin = Origin::SubAgent {
        parent: ParentRun {
            identity: Arc::new(RunIdentity {
                run_id: "run_turn".into(),
                ..Default::default()
            }),
            usage: None,
            coverage_stream: None,
            netflow_stream: None,
        },
    };
    spec.run_id = "sub_child".into();
    spec.transcript_path = home
        .global
        .join("runs/run_turn/sub/sub_child/transcript.jsonl");
    let child = Arc::new(bus.sibling("child"));
    assert!(bind(
        &home.global,
        None,
        &spec,
        child.clone(),
        "exec_child".into()
    )
    .is_err());
    spec.services.work_queue_owner = Some(parent.binding());
    let queue = bind(
        &home.global,
        None,
        &spec,
        child.clone(),
        "exec_child".into(),
    )
    .unwrap();
    let WorkActor::Agent { recording, .. } = queue.actor() else {
        panic!()
    };
    assert_eq!(recording.anchor, WorkRecordingAnchor::GlobalRuns);
    assert_eq!(
        recording.relative_path,
        "run_turn/sub/sub_child/transcript.jsonl"
    );
    spec.transcript_path = home
        .global
        .join("runs/another_parent/sub/sub_child/transcript.jsonl");
    assert!(bind(&home.global, None, &spec, child, "exec_child".into()).is_err());
}

#[test]
fn lead_round_is_explicit_and_two_rounds_keep_distinct_recordings() {
    let home = Home::new();
    let mut spec = home.session();
    let dir = home.global.join("agentiflows/af_queue");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("agentiflow.json"),
        json!({"id":"af_queue","name":"review","started_at":chrono::Utc::now(),"status":"running"})
            .to_string(),
    )
    .unwrap();
    spec.origin = Origin::FlowLead {
        flow_id: "af_queue".into(),
        flow_dir: dir.clone(),
        messaging_off: false,
    };
    spec.run_id = "af_queue".into();
    spec.transcript_path = dir.join("lead/transcript.r1.jsonl");
    let bus = Arc::new(Bus::open(&dir, "lead").staying());
    assert!(bind(&home.global, None, &spec, bus.clone(), "exec_round1".into()).is_err());
    spec.services.work_queue_round = Some(1);
    let first = bind(&home.global, None, &spec, bus.clone(), "exec_round1".into()).unwrap();
    spec.services.work_queue_round = Some(2);
    spec.transcript_path = dir.join("lead/transcript.r2.jsonl");
    let second = bind(&home.global, None, &spec, bus, "exec_round2".into()).unwrap();
    assert_ne!(first.actor(), second.actor());
}

#[tokio::test]
async fn tools_reject_forged_authority_and_record_runtime_source_structurally() {
    let home = Home::new();
    let spec = home.session();
    let (queue, bus) = home.queue(&spec, "exec_tools");
    let mut ctx = rupu_tools::ToolContext::default();
    ctx.services.message_bus = Some(bus);
    ctx.services.work_queue = Some(queue.clone());
    ctx.call = rupu_tools::CallContext {
        turn_index: Some(7),
        call_sequence: Some(3),
        tool_call_id: Some("repeated_provider_id".into()),
        ..Default::default()
    };
    let tool = rupu_tools::bodies::body("board.work_post", &ctx).unwrap();
    let input = json!({"request_id":"tool_post","expected_queue_id":null,"expected_revision":0,"title":"Review","instructions":"Inspect source"});
    for (key, value) in [
        ("actor", json!({"kind":"operator"})),
        ("run_id", json!("other")),
        ("execution_id", json!("exec_other")),
        ("recording", json!({"relative_path":"/tmp/other"})),
        ("action", json!("cancel")),
    ] {
        let mut forged = input.clone();
        forged[key] = value;
        assert!(matches!(
            tool.invoke(forged, &ctx).await,
            Err(rupu_tools::ToolError::InvalidInput(_))
        ));
    }
    let policy = rupu_tools::PermissionPolicy::unattended(PermissionMode::Readonly);
    assert!(matches!(
        policy.decide(tool.descriptor(), &input, &mut Default::default()),
        rupu_tools::Decision::Allow
    ));
    let out = tool.invoke(input, &ctx).await.unwrap();
    assert!(out.error.is_none());
    let structured = out.structured.unwrap();
    assert_eq!(
        structured,
        serde_json::from_str::<serde_json::Value>(&out.stdout).unwrap()
    );
    assert_eq!(
        structured["current_item"]["creator"]["execution_id"],
        "exec_tools"
    );
    assert_eq!(
        structured["current_item"]["latest_event"]["source"],
        json!({"turn_index":7,"call_sequence":3,"tool_call_id":"repeated_provider_id"})
    );
    let unavailable = rupu_tools::ToolContext::default();
    assert!(rupu_tools::bodies::body("board.work_post", &unavailable).is_none());
    ctx.services.message_bus = None;
    assert!(!ctx
        .services
        .provided(false)
        .contains(rupu_tools::Service::WorkQueue));
    assert!(rupu_tools::bodies::body("board.work_post", &ctx).is_none());
}

#[tokio::test]
async fn assembled_execution_stamps_duplicate_ids_and_denied_call_positions() {
    use crate::assembly::{AssemblyContext, PreparedProvider, RunAssembler};
    use rupu_agent::runner::{MockProvider, ScriptedTurn};
    use rupu_providers::types::{ContentBlock, StopReason};
    let home = Home::new();
    let mut spec = home.session();
    // Initialize through the production Queue API so model calls use the
    // same exact queue/revision contract as operator and native callers.
    let (seed, _) = home.queue(&spec, "exec_seed");
    let first = seed.apply(post("seed"), source()).unwrap();
    let queue_id = first.receipt.queue_id;
    let call = |request: &str, revision: u64| ContentBlock::ToolUse {
        id: "same_provider_id".into(),
        name: "board.work_post".into(),
        input: json!({
            "request_id":request,"expected_queue_id":queue_id,"expected_revision":revision,"title":request,"instructions":"Inspect scoped source"}),
    };
    let provider = MockProvider::new(vec![
        ScriptedTurn::AssistantBlocks {
            content: vec![
                ContentBlock::ToolUse {
                    id: "denied_write".into(),
                    name: "write_file".into(),
                    input: json!({"path":"must-not-exist","content":"denied"}),
                },
                call("first", 1),
                call("second", 2),
            ],
            stop: StopReason::ToolUse,
        },
        ScriptedTurn::AssistantBlocks {
            content: vec![call("third", 3)],
            stop: StopReason::ToolUse,
        },
        ScriptedTurn::AssistantText {
            text: "Recorded three requests".into(),
            stop: StopReason::EndTurn,
            input_tokens: 1,
            output_tokens: 1,
        },
    ]);
    spec.services.provider = Some(PreparedProvider::injected(
        "anthropic",
        "test-model",
        Box::new(provider),
        &spec.agent,
        Some(rupu_providers::model_limits::ModelLimits::unknown()),
    ));
    let transcript = spec.transcript_path.clone();
    let assembler = RunAssembler::new(AssemblyContext::minimal(home.global.clone()));
    let assembled = assembler.assemble(spec).await.unwrap();
    assert!(assembled.opts.grant.offers("board.work_post"));
    let queue = assembled
        .opts
        .tool_context
        .services
        .work_queue
        .clone()
        .unwrap();
    let WorkActor::Agent {
        ref execution_id, ..
    } = queue.actor()
    else {
        panic!()
    };
    assert_ne!(execution_id, "exec_seed");
    assert_eq!(
        assembled
            .opts
            .tool_context
            .services
            .message_bus
            .as_ref()
            .unwrap()
            .member()
            .unwrap()
            .work_execution_id
            .as_ref(),
        Some(execution_id)
    );
    let result = rupu_agent::run_agent(assembled.opts).await.unwrap();
    assert!(result.terminal_error().is_none());
    assert!(!home.global.join("must-not-exist").exists());
    let page = queue
        .list(WorkPageQuery {
            filters: Default::default(),
            limit: 20,
            cursor: None,
        })
        .unwrap();
    assert_eq!(page.counts.matching_total, 4);
    for (title, turn, sequence) in [("first", 0, 1), ("second", 0, 2), ("third", 1, 0)] {
        let item = page.items.iter().find(|i| i.title == title).unwrap();
        let item = queue.get(&item.work_id).unwrap();
        assert_eq!(
            item.latest_event.source,
            Some(WorkSource {
                turn_index: turn,
                call_sequence: sequence,
                tool_call_id: "same_provider_id".into()
            })
        );
    }
    let fresh = WorkCommand {
        request_id: "after_runner_end".into(),
        expected_queue_id: page.snapshot.queue_id,
        expected_revision: page.snapshot.revision,
        operation: WorkOperation::Post {
            title: "No".into(),
            instructions: "No".into(),
        },
    };
    assert!(queue.apply(fresh, source()).is_err());
    let events: Vec<_> = rupu_transcript::JsonlReader::iter(transcript)
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(events.iter().filter(|e| matches!(e,rupu_transcript::Event::ToolResult {structured:Some(v),..} if v.get("receipt").is_some())).count(),3);
}

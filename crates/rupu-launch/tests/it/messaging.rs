//! A launched child shares its parent's message space (spec F1a FD6, §9
//! test 4): a sub-agent is handed a bus on it, a process `agent` child is
//! started with `--message-space` / `--participant`; what either posts, the
//! parent reads on its next turn, and its `parent` is the parent.

use std::sync::Arc;

use rupu_fleet::{Address, Bus, Member, PostKind};
use rupu_runtime::argv::RunArgv;
use rupu_tools::{ChildKind, ChildState, PermissionMode, ToolContext};
use serde_json::json;
use serial_test::serial;

use crate::support::{config, request, run, Home, MockScript};

const PARENT: &str = "cobalt-harbor/heron";

/// The parent run, joined to its own space as the assembler joins it.
fn parent_in_space(home: &Home, agents: &[&str]) -> (ToolContext, Arc<Bus>) {
    let mut ctx = home.parent(Some(PARENT), agents, PermissionMode::Bypass);
    let bus = Arc::new(Bus::open(
        &home.runs().join("parent_run_1").join("messages"),
        "heron",
    ));
    bus.join(Member {
        codename: Some(PARENT.into()),
        agent: "parent".into(),
        run_id: "parent_run_1".into(),
        kind: "standalone".into(),
        ..Default::default()
    })
    .unwrap();
    ctx.services.message_bus = Some(Arc::clone(&bus));
    ctx.services.message_space = Some(Arc::clone(&bus));
    (ctx, bus)
}

/// The parent's next turn: `board.read`, through its own tool.
async fn board_read(ctx: &ToolContext) -> String {
    let out = rupu_tools::bodies::body("board.read", ctx)
        .unwrap()
        .invoke(json!({}), ctx)
        .await
        .unwrap();
    assert!(out.error.is_none(), "{out:?}");
    out.stdout
}

#[tokio::test]
#[serial]
async fn dispatch_child_shares_space() {
    let home = Home::new();
    home.agent("poster", "provider: anthropic\nmodel: claude-sonnet-4-6");
    // The process child, as `rupu run --message-space <dir> --participant
    // <p>` runs it: it joins the space under its own run, and posts.
    let backend = Arc::new(
        rupu_launch::testing::ScriptedProcesses::done("posted").with_on_spawn(|launch| {
            let RunArgv::Agent(r) = &launch.argv else {
                panic!("an agent child");
            };
            let space = r
                .message_space
                .clone()
                .expect("it joins its parent's space");
            let bus = Bus::open(&space.dir, space.participant);
            bus.join(Member {
                codename: r.codename.clone(),
                agent: r.agent.clone(),
                run_id: r.run_id.clone(),
                kind: "agent_child".into(),
                parent_run_id: r.launch.as_ref().map(|l| l.parent_run.clone()),
                ..Default::default()
            })
            .unwrap();
            let r = bus
                .post(
                    PostKind::Observation,
                    Some(&Address::Parent),
                    "port 22 is open",
                )
                .unwrap();
            assert_eq!(r.message.to.as_deref(), Some("heron"), "its parent");
            assert_eq!(r.delivery().resolved, vec!["heron".to_string()]);
        }),
    );
    let l = home.launcher_with(home.assembler(config()), backend.clone(), None);
    let (ctx, bus) = parent_in_space(&home, &["poster"]);

    let status = run(
        &l,
        &ctx,
        request(ChildKind::Agent, "poster", Some("look around")),
    )
    .await;
    assert_eq!(status.state, ChildState::Done, "{status:?}");
    let spawned = backend.spawned();
    let RunArgv::Agent(argv) = &spawned[0].argv else {
        panic!()
    };
    let space = argv.message_space.clone().unwrap();
    assert_eq!(space.dir, bus.space());
    assert_eq!(space.participant, format!("heron>{}#1", role_of(argv)));

    let read = board_read(&ctx).await;
    assert!(read.contains("port 22 is open"), "{read}");
    assert!(
        read.contains(&format!("{} [observation] -> heron", space.participant)),
        "{read}"
    );
}

/// The role word the launcher's namer gave the child (`heron>lynx#1`).
fn role_of(argv: &rupu_runtime::argv::AgentRun) -> String {
    let codename = argv.codename.clone().unwrap();
    let last = codename.rsplit('>').next().unwrap();
    last.split('#').next().unwrap().to_string()
}

#[tokio::test]
#[serial]
async fn a_subagent_shares_the_space_in_process() {
    let home = Home::new();
    home.agent(
        "poster",
        "provider: anthropic\nmodel: claude-sonnet-4-6\nmaxTurns: 3\ntools: [board.post]",
    );
    let l = home.launcher(config(), None);
    let _m = MockScript::set(
        r#"[
          { "AssistantToolUse": { "text": null, "tool_id": "p1", "tool_name": "board.post", "tool_input": {"kind": "answer", "body": "the gateway is 10.0.0.1", "addressed_to": "parent"}, "stop": "tool_use" } },
          { "AssistantText": { "text": "done", "stop": "end_turn" } }
        ]"#,
    );
    let (ctx, bus) = parent_in_space(&home, &["poster"]);
    let status = run(&l, &ctx, request(ChildKind::Subagent, "poster", Some("go"))).await;
    assert_eq!(status.state, ChildState::Done, "{status:?}");

    let read = board_read(&ctx).await;
    assert!(read.contains("the gateway is 10.0.0.1"), "{read}");
    assert!(read.contains("-> heron"), "{read}");
    // The child joined under its own run and left as it ended.
    let roster = bus.roster().unwrap();
    let child = roster
        .participants()
        .find(|p| p.member.run_id == status.run_id)
        .expect("the sub-agent joined");
    assert!(child.address.starts_with("heron>"), "{}", child.address);
    assert_eq!(child.member.parent_run_id.as_deref(), Some("parent_run_1"));
    assert!(!child.live);
}

/// A parent that takes no part in messaging (no messaging tools) still has
/// a space its children meet it in: what the child sends `parent` reaches
/// the parent.
#[tokio::test]
#[serial]
async fn a_parent_without_messaging_tools_hears_its_child() {
    let home = Home::new();
    home.agent(
        "reporter",
        "provider: anthropic\nmodel: claude-sonnet-4-6\nmaxTurns: 3\ntools: [msg.send]",
    );
    let l = home.launcher(config(), None);
    let _m = MockScript::set(
        r#"[
          { "AssistantToolUse": { "text": null, "tool_id": "m1", "tool_name": "msg.send", "tool_input": {"to": "parent", "body": "all clear"}, "stop": "tool_use" } },
          { "AssistantText": { "text": "done", "stop": "end_turn" } }
        ]"#,
    );
    let (mut ctx, bus) = parent_in_space(&home, &["reporter"]);
    ctx.services.message_bus = None;
    let status = run(
        &l,
        &ctx,
        request(ChildKind::Subagent, "reporter", Some("go")),
    )
    .await;
    assert_eq!(status.state, ChildState::Done, "{status:?}");
    let inbox = bus.mailbox.drain("heron").unwrap();
    assert_eq!(inbox.len(), 1, "{inbox:?}");
    assert_eq!(inbox[0].body, "all clear");
}

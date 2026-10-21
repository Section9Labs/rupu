use crate::*;
use std::path::Path;
use std::sync::atomic::Ordering;

fn member(agent: &str, run: &str, step: Option<&str>, parent: Option<&str>) -> Member {
    Member {
        codename: None,
        agent: agent.into(),
        run_id: run.into(),
        step_id: step.map(str::to_string),
        unit: None,
        kind: "workflow_step".into(),
        parent_run_id: parent.map(str::to_string),
        pid: None,
    }
}

fn joined(space: &Path, address: &str, m: Member) -> Bus {
    let bus = Bus::open(space, address);
    bus.join(m).unwrap();
    bus
}

fn log_lines(bus: &Bus) -> Vec<LogLine> {
    bus.read_log(None, usize::MAX).unwrap().lines
}

fn messages(bus: &Bus) -> Vec<LogMessage> {
    log_lines(bus)
        .into_iter()
        .filter_map(|l| match l {
            LogLine::Msg(m) => Some(m),
            LogLine::Seen { .. } => None,
        })
        .collect()
}

#[test]
fn log_is_superset() {
    let tmp = tempfile::tempdir().unwrap();
    let a = joined(tmp.path(), "lynx#1", member("recon", "r1", None, None));
    let b = joined(tmp.path(), "otter#1", member("triage", "r2", None, None));
    let to_b = Address::parse("otter#1").unwrap();

    // Each write appends exactly one log line, and the working state follows.
    type Write = Box<dyn Fn(&Bus) -> SendReceipt>;
    let writes: Vec<(&str, Write)> = vec![
        (
            "post",
            Box::new(|bus| bus.post(PostKind::Note, None, "p").unwrap()),
        ),
        ("direct", Box::new(move |bus| bus.send(&to_b, "d").unwrap())),
        (
            "broadcast",
            Box::new(|bus| bus.send(&Address::Broadcast, "b").unwrap()),
        ),
        (
            "directive",
            Box::new(|bus| bus.directive(None, "x").unwrap()),
        ),
    ];
    for (name, write) in &writes {
        let before = log_lines(&a).len();
        let r = write(&a);
        let after = log_lines(&a);
        assert_eq!(after.len(), before + 1, "{name}");
        assert_eq!(
            after.last(),
            Some(&LogLine::Msg(r.message.clone())),
            "{name}"
        );
    }
    let directive = a.board.read_directives().unwrap()[0].id.clone();
    let before = log_lines(&a).len();
    a.retract(&directive).unwrap();
    assert_eq!(log_lines(&a).len(), before + 1, "retract");
    assert!(a.board.read_directives().unwrap().is_empty());
    assert_eq!(a.board.read_posts().unwrap().len(), 1);
    assert_eq!(b.mailbox.drain("otter#1").unwrap().len(), 1);
    assert_eq!(b.mailbox.read_broadcast("otter#1").unwrap().len(), 1);

    // A crash between the log line and the working state leaves the line and
    // nothing else.
    a.fail_after_log.store(true, Ordering::SeqCst);
    let before = messages(&a).len();
    assert!(a.post(PostKind::Note, None, "lost").is_err());
    assert!(a.send(&Address::parse("otter#1").unwrap(), "lost").is_err());
    assert!(a.send(&Address::Broadcast, "lost").is_err());
    assert!(a.directive(None, "lost").is_err());
    let logged = messages(&a);
    assert_eq!(logged.len(), before + 4);
    assert!(logged[before..].iter().all(|m| m.body == "lost"));
    assert_eq!(a.board.read_posts().unwrap().len(), 1);
    assert!(b.mailbox.drain("otter#1").unwrap().is_empty());
    assert!(b.mailbox.read_broadcast("otter#1").unwrap().is_empty());
    assert!(a.board.read_directives().unwrap().is_empty());
    assert!(
        !log_lines(&a)
            .iter()
            .any(|l| matches!(l, LogLine::Seen { .. })),
        "nothing says the lost messages were seen"
    );
}

#[test]
fn address_resolution() {
    let tmp = tempfile::tempdir().unwrap();
    let space = tmp.path();
    let mut lynx = member("recon", "r1", Some("scan"), None);
    lynx.codename = Some("cobalt-harbor/lynx#1".into());
    joined(space, "lynx#1", lynx);
    joined(space, "heron", member("auditor", "r3", None, None))
        .leave("done")
        .unwrap();
    let sender = joined(
        space,
        "otter#1",
        member("triage", "r2", Some("fix"), Some("r1")),
    );

    let undelivered = |to: &str, reason| {
        vec![Undelivered {
            to: to.into(),
            reason,
        }]
    };
    use Undeliverable::*;
    let table: Vec<(&str, &str, Vec<&str>, Vec<Undelivered>)> = vec![
        // (to, recorded to, resolved, undelivered)
        ("lynx#1", "lynx#1", vec!["lynx#1"], vec![]),
        ("cobalt-harbor/lynx#1.2", "lynx#1", vec!["lynx#1"], vec![]),
        ("role:recon", "role:recon", vec!["lynx#1"], vec![]),
        ("recon", "recon", vec!["lynx#1"], vec![]),
        ("step:scan", "step:scan", vec!["lynx#1"], vec![]),
        ("parent", "lynx#1", vec!["lynx#1"], vec![]),
        ("lead", "lead", vec![], undelivered("lead", NoLead)),
        (
            "heron",
            "heron",
            vec![],
            undelivered("heron", ParticipantLeft),
        ),
        ("broadcast", "broadcast", vec!["lynx#1"], vec![]),
        ("operator", "operator", vec!["operator"], vec![]),
        (
            "nobody#9",
            "nobody#9",
            vec![],
            undelivered("nobody#9", UnknownParticipant),
        ),
        (
            "role:ghost",
            "role:ghost",
            vec![],
            undelivered("role:ghost", NoLiveParticipant),
        ),
    ];
    for (to, recorded, resolved, missed) in table {
        let r = sender.send(&Address::parse(to).unwrap(), "hi").unwrap();
        assert_eq!(r.message.to.as_deref(), Some(recorded), "{to}");
        assert_eq!(r.delivery().resolved, resolved, "{to}");
        assert_eq!(r.delivery().undelivered, missed, "{to}");
        assert_eq!(
            messages(&sender).last(),
            Some(&r.message),
            "{to}: logged whether or not it was delivered"
        );
    }
    // Only the live, resolved recipient got anything.
    let lynx_inbox = sender.mailbox.drain("lynx#1").unwrap();
    assert_eq!(lynx_inbox.len(), 6);
    assert!(sender.mailbox.drain("heron").unwrap().is_empty());

    // A parentless sender's `parent`.
    let orphan = joined(space, "kite", member("triage", "r4", None, None));
    let r = orphan.send(&Address::Parent, "hi").unwrap();
    assert_eq!(r.delivery().undelivered, undelivered("parent", NoParent));
}

#[test]
fn full_inboxes_and_a_full_broadcast_log_are_recorded() {
    let tmp = tempfile::tempdir().unwrap();
    joined(tmp.path(), "lynx#1", member("recon", "r1", None, None));
    let caps = BusCaps {
        msg_cap: 1,
        ..BusCaps::default()
    };
    let sender = Bus::open(tmp.path(), "otter#1").with_caps(caps);
    sender.join(member("triage", "r2", None, None)).unwrap();
    let to = Address::parse("lynx#1").unwrap();
    assert!(sender
        .send(&to, "1")
        .unwrap()
        .delivery()
        .undelivered
        .is_empty());
    let r = sender.send(&to, "2").unwrap();
    assert_eq!(
        r.delivery().undelivered,
        vec![Undelivered {
            to: "lynx#1".into(),
            reason: Undeliverable::InboxFull
        }]
    );
    sender.send(&Address::Broadcast, "1").unwrap();
    let r = sender.send(&Address::Broadcast, "2").unwrap();
    assert_eq!(
        r.delivery().undelivered[0].reason,
        Undeliverable::BroadcastFull
    );
    assert_eq!(messages(&sender).len(), 4);
}

#[test]
fn log_size_cap() {
    let tmp = tempfile::tempdir().unwrap();
    let bus = Bus::open(tmp.path(), "lynx#1").with_caps(BusCaps {
        log_max_bytes: 400,
        ..BusCaps::default()
    });
    bus.join(member("recon", "r1", None, None)).unwrap();
    let mut sent = 0;
    let err = loop {
        match bus.send(&Address::Operator, "x".repeat(50).as_str()) {
            Ok(_) => sent += 1,
            Err(e) => break e,
        }
    };
    assert!(sent >= 1);
    assert!(
        matches!(err, FleetError::LogFull { max_bytes: 400 }),
        "{err}"
    );
    assert_eq!(messages(&bus).len(), sent, "a refused send is not logged");
    // Receipts are never refused.
    bus.mark_seen(&["msg_x".into()], "lynx#1", 1).unwrap();
}

#[test]
fn a_rejoin_keeps_pending_broadcasts_and_writes_no_second_join() {
    let tmp = tempfile::tempdir().unwrap();
    let lead = Bus::open(tmp.path(), "lead").staying();
    let m = member("lead-agent", "af_1", None, None);
    lead.join(m.clone()).unwrap();
    let unit = joined(
        tmp.path(),
        "lynx#1",
        member("recon", "u1", None, Some("af_1")),
    );
    unit.send(&Address::Broadcast, "to all").unwrap();
    lead.join(m).unwrap();
    assert_eq!(lead.mailbox.read_broadcast("lead").unwrap().len(), 1);
    let joins = std::fs::read_to_string(lead.participants_path())
        .unwrap()
        .lines()
        .filter(|l| l.contains("\"address\":\"lead\""))
        .count();
    assert_eq!(joins, 1);
    // A unit's `parent` is the lead.
    let r = unit.send(&Address::Parent, "done").unwrap();
    assert_eq!(r.delivery().resolved, vec!["lead".to_string()]);
}

#[test]
fn log_pages_end_before_a_message() {
    let tmp = tempfile::tempdir().unwrap();
    let bus = joined(tmp.path(), "lynx#1", member("recon", "r1", None, None));
    let ids: Vec<String> = (0..5)
        .map(|i| {
            let r = bus.send(&Address::Operator, &i.to_string()).unwrap();
            bus.mark_seen(&[r.id().to_string()], "operator", 0).unwrap();
            r.id().to_string()
        })
        .collect();
    let mut after = None;
    let mut seen = Vec::new();
    loop {
        let page = bus.read_log(after.as_deref(), 3).unwrap();
        assert!(matches!(page.lines.first(), Some(LogLine::Msg(_))));
        seen.extend(page.lines);
        match page.next {
            Some(n) => after = Some(n),
            None => break,
        }
    }
    assert_eq!(seen, log_lines(&bus), "every line on exactly one page");
    assert_eq!(ids.len(), 5);
}

#[test]
fn legacy_flow_reads() {
    let tmp = tempfile::tempdir().unwrap();
    let flow = tmp.path().join("agentiflows").join("af_old");
    // A flow written before the log and the registry: the old post shape and
    // a unit's sanitized inbox directory.
    std::fs::create_dir_all(flow.join("board")).unwrap();
    std::fs::write(
        flow.join("board").join("posts.jsonl"),
        "{\"author\":\"recon#1\",\"ts\":\"t\",\"kind\":\"observation\",\"body\":\"old post\"}\n",
    )
    .unwrap();
    std::fs::create_dir_all(flow.join("mailboxes").join("recon-1")).unwrap();

    let lead = Bus::open(&flow, "lead");
    assert_eq!(lead.board.read_posts().unwrap()[0].body, "old post");
    assert!(lead.read_log(None, 10).unwrap().lines.is_empty());
    let r = lead
        .send(&Address::parse("recon#1").unwrap(), "hello")
        .unwrap();
    assert_eq!(r.delivery().resolved, vec!["recon#1".to_string()]);
    assert!(flow.join("mailboxes/recon-1/inbox.jsonl").exists());
    assert_eq!(lead.mailbox.drain("recon#1").unwrap()[0].body, "hello");
    let r = lead
        .send(&Address::parse("recon#2").unwrap(), "nobody")
        .unwrap();
    assert_eq!(
        r.delivery().undelivered[0].reason,
        Undeliverable::UnknownParticipant
    );
}

#[test]
fn parent_resolves_by_run_after_a_later_run_takes_the_address() {
    let tmp = tempfile::tempdir().unwrap();
    // A session's first turn, its child, then the session's next turn.
    joined(
        tmp.path(),
        "heron",
        member("assistant", "turn_1", None, None),
    );
    let child = joined(
        tmp.path(),
        "heron>otter#1",
        member("helper", "c1", None, Some("turn_1")),
    );
    joined(
        tmp.path(),
        "heron",
        member("assistant", "turn_2", None, None),
    );
    let r = child.send(&Address::Parent, "done").unwrap();
    assert_eq!(r.delivery().resolved, vec!["heron".to_string()]);
}

#[test]
fn a_legacy_inbox_resolves_after_new_participants_join() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("mailboxes").join("recon-1")).unwrap();
    let lead = joined(tmp.path(), "lead", member("lead-agent", "af", None, None));
    let r = lead
        .send(&Address::parse("recon#1").unwrap(), "hi")
        .unwrap();
    assert_eq!(r.delivery().resolved, vec!["recon#1".to_string()]);
}

#[test]
fn broadcast_as_a_directive_or_post_address_means_everyone() {
    let tmp = tempfile::tempdir().unwrap();
    joined(tmp.path(), "lynx#1", member("recon", "r1", None, None));
    let lead = joined(tmp.path(), "lead", member("lead-agent", "af", None, None));
    let d = lead.directive(Some(&Address::Broadcast), "hold").unwrap();
    assert_eq!(d.message.to, None);
    assert_eq!(lead.board.read_directives().unwrap()[0].addressed_to, None);
    let p = lead
        .post(PostKind::Note, Some(&Address::Broadcast), "hi")
        .unwrap();
    assert_eq!(p.message.to, None);
}

#[test]
fn a_failed_delivery_after_the_log_line_says_so() {
    let tmp = tempfile::tempdir().unwrap();
    joined(tmp.path(), "lynx#1", member("recon", "r1", None, None));
    let lead = joined(tmp.path(), "lead", member("lead-agent", "af", None, None));
    lead.fail_after_log.store(true, Ordering::SeqCst);
    let err = lead
        .send(&Address::parse("lynx#1").unwrap(), "x")
        .unwrap_err();
    let id = messages(&lead).last().unwrap().id.clone();
    assert!(err.to_string().contains(&id), "{err}");
    assert!(err.to_string().contains("don't send it again"), "{err}");
}

fn operator(to: &str, body: &str, urgent: bool) -> OperatorMessage {
    OperatorMessage {
        to: Address::parse(to).unwrap(),
        body: body.into(),
        urgent,
        via: OperatorVia::Cp,
        viewer: Some("matt".into()),
        client: None,
    }
}

#[test]
fn a_line_without_client_still_parses_and_round_trips_without_it() {
    let old = serde_json::json!({
        "id": "msg_1", "ts": "2026-01-01T00:00:00Z", "kind": "operator",
        "from": "operator", "to": "lead", "body": "hi",
        "delivery": {"resolved": ["lead"]}, "via": "cp"
    });
    let m: LogMessage = serde_json::from_value(old).unwrap();
    assert_eq!(m.client, None);
    let back = serde_json::to_value(&m).unwrap();
    assert!(back.get("client").is_none(), "{back}");

    let mut with = back.clone();
    with["client"] = serde_json::json!("mcp");
    let m: LogMessage = serde_json::from_value(with).unwrap();
    assert_eq!(m.client.as_deref(), Some("mcp"));
    assert_eq!(serde_json::to_value(&m).unwrap()["client"], "mcp");
}

#[test]
fn operator_send_logs_an_operator_line_and_delivers_it() {
    let tmp = tempfile::tempdir().unwrap();
    let lynx = joined(
        tmp.path(),
        "lynx#1",
        member("recon", "r1", Some("scan"), None),
    );
    let otter = joined(tmp.path(), "otter#1", member("triage", "r2", None, None));
    let op = Bus::open(tmp.path(), "operator");

    let r = op
        .operator_send(operator("step:scan", "focus on 10.0.0.5", true))
        .unwrap();
    let logged = serde_json::to_value(&messages(&op)[0]).unwrap();
    assert_eq!(
        logged,
        serde_json::json!({
            "id": r.id(), "ts": r.message.ts, "kind": "operator", "from": "operator",
            "to": "step:scan", "body": "focus on 10.0.0.5", "urgent": true,
            "delivery": {"resolved": ["lynx#1"]}, "via": "cp", "viewer": "matt"
        })
    );
    let inbox = lynx.mailbox.drain("lynx#1").unwrap();
    assert_eq!(inbox.len(), 1);
    assert_eq!(
        (
            inbox[0].from.as_str(),
            inbox[0].body.as_str(),
            inbox[0].urgent
        ),
        ("operator", "focus on 10.0.0.5", true)
    );
    assert!(otter.mailbox.drain("otter#1").unwrap().is_empty());

    let r = op
        .operator_send(operator("broadcast", "wrap up", false))
        .unwrap();
    assert_eq!(r.delivery().resolved, vec!["lynx#1", "otter#1"]);
    for (bus, me) in [(&lynx, "lynx#1"), (&otter, "otter#1")] {
        let got = bus.mailbox.read_broadcast(me).unwrap();
        assert_eq!(got.len(), 1, "{me}");
        assert_eq!((got[0].from.as_str(), got[0].urgent), ("operator", false));
    }
    assert!(!op.is_joined(), "the operator never joins");
    assert!(op.roster().unwrap().get("operator").is_none());
}

#[test]
fn only_operator_send_writes_from_operator() {
    let tmp = tempfile::tempdir().unwrap();
    joined(tmp.path(), "lynx#1", member("recon", "r1", None, None));
    let op = Bus::open(tmp.path(), "operator");
    let to = Address::parse("lynx#1").unwrap();
    assert!(matches!(op.send(&to, "obey"), Err(FleetError::Refused(_))));
    assert!(matches!(
        op.post(PostKind::Note, None, "obey"),
        Err(FleetError::Refused(_))
    ));
    assert!(matches!(
        op.directive(None, "obey"),
        Err(FleetError::Refused(_))
    ));
    assert!(matches!(op.retract("msg_x"), Err(FleetError::Refused(_))));
    assert!(matches!(
        op.operator_send(operator("operator", "hi", false)),
        Err(FleetError::Refused(_))
    ));
    assert!(messages(&op).is_empty());
}

#[test]
fn send_to_left_participant_logged() {
    let tmp = tempfile::tempdir().unwrap();
    joined(tmp.path(), "lynx#1", member("recon", "r1", None, None))
        .leave("done")
        .unwrap();
    let op = Bus::open(tmp.path(), "operator");
    let r = op
        .operator_send(operator("lynx#1", "are you there?", false))
        .unwrap();
    assert!(r.delivery().resolved.is_empty());
    assert_eq!(
        r.delivery().undelivered,
        vec![Undelivered {
            to: "lynx#1".into(),
            reason: Undeliverable::ParticipantLeft
        }]
    );
    let logged = messages(&op);
    assert_eq!(logged.len(), 1);
    assert_eq!(logged[0].kind, MessageKind::Operator);
    assert!(!tmp.path().join("mailboxes/lynx_1/inbox.jsonl").exists());
}

#[test]
fn a_kind_parses_from_the_name_it_serializes_as() {
    let names: Vec<String> = MessageKind::ALL
        .iter()
        .map(|k| {
            serde_json::to_value(k)
                .unwrap()
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();
    assert_eq!(
        names,
        [
            "post",
            "direct",
            "broadcast",
            "directive",
            "retract",
            "operator"
        ]
    );
    for (k, name) in MessageKind::ALL.iter().zip(&names) {
        assert_eq!(MessageKind::parse(name), Some(*k));
    }
    assert_eq!(MessageKind::parse("Direct"), None);
}

#[test]
fn a_broadcast_into_an_empty_room_is_undelivered() {
    let tmp = tempfile::tempdir().unwrap();
    let alone = joined(tmp.path(), "lynx#1", member("recon", "r1", None, None));
    let op = Bus::open(tmp.path(), "operator");
    let empty_room = vec![Undelivered {
        to: "broadcast".into(),
        reason: Undeliverable::NoLiveParticipant,
    }];
    let r = alone.send(&Address::Broadcast, "anyone?").unwrap();
    assert!(r.delivery().resolved.is_empty());
    assert_eq!(r.delivery().undelivered, empty_room);
    alone.leave("done").unwrap();
    let r = op
        .operator_send(operator("broadcast", "anyone?", false))
        .unwrap();
    assert_eq!(r.delivery().undelivered, empty_room);
    assert_eq!(messages(&op).len(), 2, "both are logged");
    assert_eq!(
        op.mailbox.broadcast_len().unwrap(),
        0,
        "nothing reached the broadcast log"
    );
}

#[test]
fn no_run_joins_at_a_fixed_address_but_lead() {
    let tmp = tempfile::tempdir().unwrap();
    for fixed in ["operator", "broadcast", "parent"] {
        let bus = Bus::open(tmp.path(), fixed);
        assert!(
            matches!(
                bus.join(member("recon", "r1", None, None)),
                Err(FleetError::Refused(_))
            ),
            "{fixed}"
        );
        assert!(!bus.is_joined(), "{fixed}");
    }
    assert!(Bus::open(tmp.path(), "operator")
        .roster()
        .unwrap()
        .is_empty());
    joined(tmp.path(), "lead", member("lead", "af_1", None, None));
}

#[test]
fn leave_all_ends_every_live_participant_once() {
    let tmp = tempfile::tempdir().unwrap();
    assert_eq!(Bus::leave_all(tmp.path(), "stopped").unwrap(), 0);
    assert!(
        !tmp.path().join("participants.jsonl").exists(),
        "a space with no roster stays unwritten"
    );
    joined(tmp.path(), "lynx#1", member("recon", "r1", None, None));
    joined(tmp.path(), "otter#1", member("triage", "r2", None, None))
        .leave("done")
        .unwrap();
    let staying = Bus::open(tmp.path(), "heron").staying();
    staying.join(member("reader", "r3", None, None)).unwrap();

    assert_eq!(Bus::leave_all(tmp.path(), "stopped").unwrap(), 2);
    let roster = staying.roster().unwrap();
    assert!(roster.participants().all(|p| !p.live));
    let lefts: Vec<(String, String)> = std::fs::read_to_string(staying.participants_path())
        .unwrap()
        .lines()
        .filter_map(|l| match serde_json::from_str(l).unwrap() {
            ParticipantEvent::Left {
                address, status, ..
            } => Some((address, status)),
            ParticipantEvent::Joined { .. } => None,
        })
        .collect();
    assert_eq!(
        lefts,
        [
            ("otter#1".to_string(), "done".to_string()),
            ("lynx#1".to_string(), "stopped".to_string()),
            ("heron".to_string(), "stopped".to_string()),
        ]
    );
    assert_eq!(Bus::leave_all(tmp.path(), "stopped").unwrap(), 0);
}

#[test]
fn an_addressed_post_reaches_the_addressee_inbox_and_the_board() {
    let tmp = tempfile::tempdir().unwrap();
    let lynx = joined(tmp.path(), "lynx#1", member("recon", "r1", None, None));
    let otter = joined(tmp.path(), "otter#1", member("triage", "r2", None, None));
    let to = Address::parse("lynx#1").unwrap();
    let r = otter
        .post(PostKind::Question, Some(&to), "seen 10.0.0.5?")
        .unwrap();
    assert_eq!(r.delivery().resolved, vec!["lynx#1".to_string()]);
    assert!(r.delivery().undelivered.is_empty());

    // In lynx's inbox, marked as a post with its kind and the log id.
    let inbox = lynx.mailbox.drain("lynx#1").unwrap();
    assert_eq!(inbox.len(), 1);
    assert_eq!(inbox[0].post_kind, Some(PostKind::Question));
    assert_eq!(inbox[0].id.as_deref(), Some(r.message.id.as_str()));
    assert_eq!(inbox[0].body, "seen 10.0.0.5?");

    // Still on the board for everyone.
    let posts = otter.board.read_posts().unwrap();
    assert_eq!(posts.len(), 1);
    assert_eq!(posts[0].addressed_to.as_deref(), Some("lynx#1"));
    // The sender's own inbox stays empty.
    assert!(otter.mailbox.drain("otter#1").unwrap().is_empty());
}

#[test]
fn a_channel_post_reaches_no_inbox() {
    let tmp = tempfile::tempdir().unwrap();
    let lynx = joined(tmp.path(), "lynx#1", member("recon", "r1", None, None));
    let otter = joined(tmp.path(), "otter#1", member("triage", "r2", None, None));
    let r = otter
        .post(PostKind::Note, None, "status: scanning")
        .unwrap();
    assert!(r.delivery().resolved.is_empty());
    assert!(lynx.mailbox.drain("lynx#1").unwrap().is_empty());
    assert_eq!(otter.board.read_posts().unwrap().len(), 1);
}

#[test]
fn an_addressed_post_to_a_role_reaches_every_live_member_and_respects_inbox_caps() {
    let tmp = tempfile::tempdir().unwrap();
    let a = joined(tmp.path(), "lynx#1", member("recon", "r1", None, None));
    let b = joined(tmp.path(), "lynx#2", member("recon", "r2", None, None));
    let caps = BusCaps {
        msg_cap: 1,
        ..BusCaps::default()
    };
    let sender = Bus::open(tmp.path(), "otter#1").with_caps(caps);
    sender.join(member("triage", "r3", None, None)).unwrap();
    // Fill lynx#2's inbox first.
    sender
        .send(&Address::parse("lynx#2").unwrap(), "busy")
        .unwrap();

    let r = sender
        .post(
            PostKind::Observation,
            Some(&Address::parse("role:recon").unwrap()),
            "found it",
        )
        .unwrap();
    assert_eq!(r.delivery().resolved, vec!["lynx#1".to_string()]);
    assert_eq!(
        r.delivery().undelivered,
        vec![Undelivered {
            to: "lynx#2".into(),
            reason: Undeliverable::InboxFull
        }]
    );
    assert_eq!(a.mailbox.drain("lynx#1").unwrap().len(), 1);
    // lynx#2 has only the earlier direct message; the post is on the board.
    let b_inbox = b.mailbox.drain("lynx#2").unwrap();
    assert_eq!(b_inbox.len(), 1);
    assert_eq!(b_inbox[0].post_kind, None);
    assert_eq!(sender.board.read_posts().unwrap().len(), 1);
}

fn journal(root: &Path) -> Vec<serde_json::Value> {
    let text = std::fs::read_to_string(root.join("board").join("claims.jsonl")).unwrap();
    text.lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[test]
fn claim_release_writes_journal_lines() {
    let tmp = tempfile::tempdir().unwrap();
    let board = Board::new(tmp.path());
    let ClaimOutcome::Granted(guard) = board
        .claim("host:1", "lynx#1", std::time::Duration::from_secs(60))
        .unwrap()
    else {
        panic!("granted")
    };
    drop(guard);
    let lines = journal(tmp.path());
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["ev"], "claimed");
    assert_eq!(lines[0]["key"], "host:1");
    assert_eq!(lines[0]["owner"], "lynx#1");
    assert!(lines[0]["lease_expires_at"].is_string());
    assert_eq!(lines[1]["ev"], "released");
    assert_eq!(lines[1]["key"], "host:1");
    assert_eq!(lines[1]["owner"], "lynx#1");
    assert!(chrono::DateTime::parse_from_rfc3339(lines[1]["at"].as_str().unwrap()).is_ok());
    let view = board.claims_view(chrono::Utc::now()).unwrap();
    assert_eq!(view.len(), 1);
    assert_eq!(view[0].state, ClaimState::Released);
    assert!(view[0].ended_at.is_some());
    assert_eq!(view[0].key, "host:1");
}

#[test]
fn expired_claim_is_reported_expired() {
    let tmp = tempfile::tempdir().unwrap();
    let board = Board::new(tmp.path());
    let ClaimOutcome::Granted(old) = board
        .claim("svc", "a", std::time::Duration::from_secs(0))
        .unwrap()
    else {
        panic!("granted")
    };
    let ClaimOutcome::Granted(_new) = board
        .claim("svc", "b", std::time::Duration::from_secs(60))
        .unwrap()
    else {
        panic!("granted")
    };
    let evs: Vec<String> = journal(tmp.path())
        .iter()
        .map(|l| {
            format!(
                "{}:{}",
                l["ev"].as_str().unwrap(),
                l["owner"].as_str().unwrap()
            )
        })
        .collect();
    assert_eq!(evs, ["claimed:a", "expired:a", "claimed:b"]);
    // The stale guard's drop finds a successor's file and journals nothing.
    drop(old);
    assert_eq!(
        journal(tmp.path()).len(),
        3,
        "no released for the stale guard"
    );
    let view = board.claims_view(chrono::Utc::now()).unwrap();
    assert_eq!(view.len(), 2);
    assert_eq!(
        (view[0].owner.as_str(), view[0].state),
        ("a", ClaimState::Expired)
    );
    assert_eq!(
        (view[1].owner.as_str(), view[1].state),
        ("b", ClaimState::Held)
    );
}

#[test]
fn pre_journal_live_claim_reads_as_held() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("board").join("claims");
    std::fs::create_dir_all(&dir).unwrap();
    let now = chrono::Utc::now();
    let rec = serde_json::json!({
        "owner": "old#1",
        "acquired_at": now.to_rfc3339(),
        "lease_expires_at": (now + chrono::Duration::hours(1)).to_rfc3339(),
        "token": "t",
    });
    std::fs::write(dir.join("k-0123456789abcdef.json"), rec.to_string()).unwrap();
    let board = Board::new(tmp.path());
    assert!(!board.has_journal());
    let view = board.claims_view(now).unwrap();
    assert_eq!(view.len(), 1);
    assert_eq!(view[0].state, ClaimState::Held);
    assert_eq!(view[0].owner, "old#1");
    assert!(!view[0].key_known);
    assert_eq!(view[0].key, "k-0123456789abcdef");
    // The same claim later, after its lease: reported expired.
    let later = board.claims_view(now + chrono::Duration::hours(2)).unwrap();
    assert_eq!(later[0].state, ClaimState::Expired);
}

#[test]
fn retraction_records_its_time() {
    let tmp = tempfile::tempdir().unwrap();
    let board = Board::new(tmp.path());
    let id = board
        .put_directive(&Directive {
            id: String::new(),
            author: "lead".into(),
            ts: "t".into(),
            body: "b".into(),
            addressed_to: None,
        })
        .unwrap();
    board.retract_directive(&id).unwrap();
    let events = board.directive_events().unwrap();
    match &events[1] {
        DirectiveEvent::Retract { retract, at } => {
            assert_eq!(retract, &id);
            assert!(chrono::DateTime::parse_from_rfc3339(at.as_deref().unwrap()).is_ok());
        }
        other => panic!("expected a retraction, got {other:?}"),
    }
    assert!(board.read_directives().unwrap().is_empty());
}

#[test]
fn a_corrupt_claim_file_does_not_fail_the_view() {
    let tmp = tempfile::tempdir().unwrap();
    let board = Board::new(tmp.path());
    let ClaimOutcome::Granted(_g) = board
        .claim("ok", "a", std::time::Duration::from_secs(60))
        .unwrap()
    else {
        panic!("granted")
    };
    let dir = tmp.path().join("board").join("claims");
    std::fs::write(dir.join("empty-0000000000000000.json"), "").unwrap();
    std::fs::write(dir.join("junk-1111111111111111.json"), "{not json").unwrap();
    let view = board.claims_view(chrono::Utc::now()).unwrap();
    assert_eq!(view.len(), 1);
    assert_eq!(view[0].key, "ok");
    assert!(view[0].key_known);
}

#[test]
fn a_half_written_last_journal_line_is_ignored() {
    let tmp = tempfile::tempdir().unwrap();
    let board = Board::new(tmp.path());
    let ClaimOutcome::Granted(g) = board
        .claim("k", "a", std::time::Duration::from_secs(60))
        .unwrap()
    else {
        panic!("granted")
    };
    std::mem::forget(g);
    let path = tmp.path().join("board").join("claims.jsonl");
    let mut text = std::fs::read_to_string(&path).unwrap();
    // A release still being written: complete JSON, but no newline yet.
    text.push_str(r#"{"ev":"released","key":"k","owner":"a","at":"2026-01-01T00:00:00+00:00"}"#);
    std::fs::write(&path, text).unwrap();
    let view = board.claims_view(chrono::Utc::now()).unwrap();
    assert_eq!(view.len(), 1);
    assert_eq!(view[0].state, ClaimState::Held);
}

#[test]
fn an_end_matches_its_own_grant_not_the_owners_newer_one() {
    let tmp = tempfile::tempdir().unwrap();
    let board = Board::new(tmp.path());
    let ttl = std::time::Duration::from_secs(60);
    let ClaimOutcome::Granted(first) = board.claim("k", "a", ttl).unwrap() else {
        panic!("granted")
    };
    // The same owner acquires the key again (its first lease having lapsed
    // and been reaped) while the first guard's release is still in flight:
    // simulate by journaling the new grant before the old release lands.
    let first_token = first.token.clone();
    std::mem::forget(first);
    std::fs::remove_dir_all(tmp.path().join("board").join("claims")).unwrap();
    let ClaimOutcome::Granted(second) = board.claim("k", "a", ttl).unwrap() else {
        panic!("granted")
    };
    board.journal_claim(&crate::board::JournalLine::Released {
        key: "k".into(),
        owner: "a".into(),
        token: Some(first_token),
        at: chrono::Utc::now().to_rfc3339(),
    });
    let view = board.claims_view(chrono::Utc::now()).unwrap();
    assert_eq!(view.len(), 2);
    let states: Vec<ClaimState> = view.iter().map(|v| v.state).collect();
    assert!(states.contains(&ClaimState::Released) && states.contains(&ClaimState::Held));
    let held = view.iter().find(|v| v.state == ClaimState::Held).unwrap();
    let released = view
        .iter()
        .find(|v| v.state == ClaimState::Released)
        .unwrap();
    assert!(held.acquired_at >= released.acquired_at);
    drop(second);
}

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
fn a_prepared_run_joins_on_its_first_write_and_creates_nothing_before() {
    let tmp = tempfile::tempdir().unwrap();
    let space = tmp.path().join("messages");
    let bus = Bus::open(&space, "heron");
    bus.prepare(member("auditor", "r1", None, None));
    assert!(!space.exists(), "nothing until the run writes");
    assert!(!bus.is_joined());
    bus.leave("done").unwrap();
    assert!(!space.exists(), "a run that never joined never leaves");

    bus.post(PostKind::Note, None, "first").unwrap();
    assert!(bus.is_joined());
    assert!(bus.roster().unwrap().get("heron").unwrap().live);
    bus.leave("done").unwrap();
    assert!(!bus.roster().unwrap().get("heron").unwrap().live);
}

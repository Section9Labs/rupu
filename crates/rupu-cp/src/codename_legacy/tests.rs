use super::*;
use rupu_orchestrator::runs::RunRecord;
use std::io::Write as _;

/// Crew `jade-reef` (pinned in rupu-codename's golden tests).
const RUN: &str = "run_01J9ZQ3K4M5N6P7Q8R9S0T1V2W";

/// Every static-slot shape the runner names: two linear steps sharing a def,
/// a `for_each` fan-out, a panel with a repeated panelist + `fix_with` fixer,
/// a `parallel:` step, and an approval gate with an `on_reject` cleanup.
const WF: &str = r#"
name: legacy-fixture
steps:
  - id: alpha
    agent: ag
    actions: []
    prompt: "a"
  - id: beta
    agent: ag
    actions: []
    prompt: "b"
  - id: fan
    agent: triage
    actions: []
    for_each: "{{ inputs.items }}"
    prompt: "c"
  - id: review
    actions: []
    panel:
      panelists: [sec, perf, sec]
      subject: "x"
      gate:
        until_no_findings_at_severity_or_above: high
        fix_with: fixer
        max_iterations: 3
  - id: par
    actions: []
    parallel:
      - id: p1
        agent: ag
        prompt: "p"
      - id: p2
        agent: triage
        prompt: "q"
  - id: gate
    approval:
      prompt: "ok?"
      on_reject:
        - id: cleanup
          agent: worker
          prompt: "undo"
"#;

/// What a NEW run of `WF` mints, computed with the runner's own naming API
/// and its rules (runner.rs: `step_codename`, `n.unit(.., idx)`,
/// `n.panelist(.., occurrence)` with occurrence only for a repeated def,
/// `n.fixer`, `n.sub` for parallel + on_reject).
fn minted() -> RunNaming {
    RunNaming::open(&Workflow::parse(WF).unwrap(), RUN, None)
}

fn namer() -> LegacyNamer {
    LegacyNamer::from_snapshot(RUN, Some("legacy-fixture"), Some(WF))
}

#[test]
fn derived_names_equal_what_the_runner_mints() {
    let m = minted();
    let mut d = namer();

    // Linear steps sharing a def get distinct roles.
    assert_eq!(
        d.step("alpha", None),
        Some(m.step("alpha", "ag").to_string())
    );
    assert_eq!(d.step("beta", None), Some(m.step("beta", "ag").to_string()));
    assert_eq!(d.step("alpha", None).unwrap(), "jade-reef/hedgehog");
    assert_eq!(d.step("beta", None).unwrap(), "jade-reef/heron");
    // Fan-out / panel / parallel steps name no single member.
    assert_eq!(d.step("fan", None), None);
    assert_eq!(d.step("review", None), None);
    assert_eq!(d.step("par", None), None);
    // Agent-less gate step has no member; its on_reject cleanup is a sub slot.
    assert_eq!(d.step("gate", None), None);
    assert_eq!(
        d.step("cleanup", None),
        Some(m.sub("gate", "cleanup", "worker").to_string())
    );

    // for_each unit: #n = index + 1 (records and events agree).
    for i in [0usize, 1, 411] {
        let want = m.unit("fan", "triage", i).to_string();
        assert_eq!(
            d.item("fan", i, "", false, None).as_deref(),
            Some(want.as_str())
        );
        assert_eq!(
            d.unit_event("fan", i, "src/a.rs", Some("triage"))
                .as_deref(),
            Some(want.as_str())
        );
    }
    assert_eq!(
        d.item("fan", 411, "", false, None).unwrap(),
        "jade-reef/numbat#412"
    );

    // Panel [sec, perf, sec]: sec#1, perf, sec#2; fixer is its own slot.
    let sec1 = m.panelist("review", "sec", Some(1)).to_string();
    let perf = m.panelist("review", "perf", None).to_string();
    let sec2 = m.panelist("review", "sec", Some(2)).to_string();
    let fixer = m.fixer("review", "fixer").to_string();
    assert!(sec1.ends_with("#1") && sec2.ends_with("#2") && !perf.contains('#'));
    assert_eq!(d.item("review", 0, "sec", false, None), Some(sec1.clone()));
    assert_eq!(d.item("review", 1, "perf", false, None), Some(perf.clone()));
    assert_eq!(d.item("review", 2, "sec", false, None), Some(sec2.clone()));
    assert_eq!(
        d.item("review", 3, "fixer:iter1", true, None),
        Some(fixer.clone())
    );
    // A pre-`is_fixer` fixer record is still recognised by its sub_id.
    assert_eq!(
        d.item("review", 3, "fixer:iter1", false, None),
        Some(fixer.clone())
    );

    // Panel events: monotonic view index across iterations; the fixer's key
    // is `iter{n}:fix:{agent}`. Gate iterations keep the same names.
    let seq = [
        (0, "iter1:sec", &sec1),
        (1, "iter1:perf", &perf),
        (2, "iter1:sec", &sec2),
        (3, "iter1:fix:fixer", &fixer),
        (4, "iter2:sec", &sec1),
        (5, "iter2:perf", &perf),
        (6, "iter2:sec", &sec2),
    ];
    for (idx, key, want) in seq {
        let (_, rest) = split_panel_key(key);
        let agent = rest.strip_prefix("fix:").unwrap_or(rest);
        assert_eq!(
            d.unit_event("review", idx, key, Some(agent)).as_deref(),
            Some(want.as_str()),
            "{key}"
        );
    }

    // Parallel sub-steps.
    assert_eq!(
        d.item("par", 0, "p1", false, None),
        Some(m.sub("par", "p1", "ag").to_string())
    );
    assert_eq!(
        d.item("par", 1, "p2", false, None),
        Some(m.sub("par", "p2", "triage").to_string())
    );
    assert_eq!(
        d.parallel_subs("par"),
        vec![
            (0, m.sub("par", "p1", "ag").to_string(), "ag".to_string()),
            (
                1,
                m.sub("par", "p2", "triage").to_string(),
                "triage".to_string()
            ),
        ]
    );

    // Every derived instance name is distinct.
    let all: std::collections::HashSet<String> = [
        d.step("alpha", None),
        d.step("beta", None),
        d.step("cleanup", None),
        Some(sec1),
        Some(perf),
        Some(sec2),
        Some(fixer),
        d.item("par", 0, "p1", false, None),
        d.item("par", 1, "p2", false, None),
    ]
    .into_iter()
    .flatten()
    .collect();
    assert_eq!(all.len(), 9);
}

#[test]
fn missing_or_unparseable_workflow_falls_back_to_derive_legacy() {
    for yaml in [None, Some(""), Some("not: [a workflow")] {
        let mut d = LegacyNamer::from_snapshot(RUN, Some("wf"), yaml);
        assert_eq!(
            d.step("anything", Some("triage")).as_deref(),
            Some(rupu_codename::derive_legacy(RUN, Some("triage")).as_str())
        );
        assert_eq!(d.step("anything", None), None, "no agent, no name");
        assert_eq!(
            d.item("fan", 3, "", false, Some("triage")).as_deref(),
            Some("jade-reef/numbat#4")
        );
        assert_eq!(
            d.unit_event("fan", 0, "k", Some("triage")).as_deref(),
            Some("jade-reef/numbat#1")
        );
    }
    // `agent:<name>` standalone runs name their one agent.
    let d = LegacyNamer::from_snapshot(RUN, Some("agent:triage"), Some(""));
    assert_eq!(d.step("agent", None).as_deref(), Some("jade-reef/numbat"));
}

#[test]
fn fill_step_records_names_only_missing_and_flags_them() {
    let m = minted();
    let d = namer();
    let mut steps = vec![
        serde_json::json!({"step_id": "alpha", "kind": "linear", "run_id": "run_a"}),
        serde_json::json!({"step_id": "beta", "kind": "linear", "run_id": "run_b",
                           "codename": "stored-name/x"}),
        serde_json::json!({"step_id": "fan", "kind": "for_each", "run_id": "",
        "items": [
            {"index": 0, "sub_id": "", "run_id": "run_u0"},
            {"index": 1, "sub_id": "", "run_id": "run_u1", "codename": "kept/y#2"}
        ]}),
        serde_json::json!({"step_id": "review", "kind": "panel", "run_id": "",
        "items": [
            {"index": 0, "sub_id": "sec", "output": "[{\"title\":\"SQLi\"}]"},
            {"index": 1, "sub_id": "perf", "output": "[]"},
            {"index": 2, "sub_id": "sec", "output": "[{\"title\":\"XSS\"}]"}
        ],
        "findings": [
            {"source": "perf", "title": "slow"},
            {"source": "sec", "title": "XSS"},
            {"source": "sec", "title": "unmatched"}
        ]}),
    ];
    d.fill_step_records(&mut steps);

    assert_eq!(steps[0]["codename"], m.step("alpha", "ag").to_string());
    assert_eq!(steps[0][DERIVED_KEY], true);
    // Stored names are untouched (and get no flag).
    assert_eq!(steps[1]["codename"], "stored-name/x");
    assert!(steps[1].get(DERIVED_KEY).is_none());
    // Fan-out step records stay unnamed by design; their items are named.
    assert!(steps[2].get("codename").is_none());
    assert_eq!(
        steps[2]["items"][0]["codename"],
        m.unit("fan", "triage", 0).to_string()
    );
    assert_eq!(steps[2]["items"][0][DERIVED_KEY], true);
    assert_eq!(steps[2]["items"][1]["codename"], "kept/y#2");
    assert!(steps[2]["items"][1].get(DERIVED_KEY).is_none());
    // Panel findings: singleton panelist by source; a repeated panelist by
    // the one item whose output carries the title; ambiguous stays unnamed.
    let f = &steps[3]["findings"];
    assert_eq!(
        f[0]["codename"],
        m.panelist("review", "perf", None).to_string()
    );
    assert_eq!(f[0][DERIVED_KEY], true);
    assert_eq!(
        f[1]["codename"],
        m.panelist("review", "sec", Some(2)).to_string()
    );
    assert!(f[2].get("codename").is_none());
}

#[test]
fn fill_units_names_panel_units_in_index_order() {
    let m = minted();
    let mut d = namer();
    // Events-only panel units, deliberately out of order.
    let mut units = vec![
        serde_json::json!({"step_id": "review", "index": 2, "item": "iter1:sec"}),
        serde_json::json!({"step_id": "review", "index": 0, "item": "iter1:sec"}),
        serde_json::json!({"step_id": "review", "index": 1, "item": "iter1:perf"}),
        serde_json::json!({"step_id": "fan", "index": 0, "item": "a.rs", "codename": "kept"}),
    ];
    d.fill_units(&mut units);
    assert_eq!(
        units[0]["codename"],
        m.panelist("review", "sec", Some(2)).to_string()
    );
    assert_eq!(
        units[1]["codename"],
        m.panelist("review", "sec", Some(1)).to_string()
    );
    assert_eq!(
        units[2]["codename"],
        m.panelist("review", "perf", None).to_string()
    );
    assert_eq!(units[1][DERIVED_KEY], true);
    assert_eq!(units[3]["codename"], "kept");
    assert!(units[3].get(DERIVED_KEY).is_none());
}

// ── Store-backed: sub-agents + events ───────────────────────────────────────

pub(crate) fn legacy_record(id: &str, workflow_name: &str) -> RunRecord {
    serde_json::from_value(serde_json::json!({
        "id": id,
        "workflow_name": workflow_name,
        "status": "completed",
        "inputs": {},
        "workspace_id": "ws_test",
        "workspace_path": "/tmp/ws",
        "transcript_dir": "/tmp/ws/.rupu/transcripts",
        "started_at": "2026-09-10T19:34:14Z",
    }))
    .expect("legacy run record")
}

pub(crate) fn write_run_start(path: &Path, agent: &str, codename: Option<&str>) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let ev = rupu_transcript::Event::RunStart {
        run_id: "x".into(),
        workspace_id: "ws".into(),
        agent: agent.into(),
        provider: "anthropic".into(),
        model: "claude-x".into(),
        started_at: chrono::Utc::now(),
        mode: rupu_transcript::RunMode::Bypass,
        schema: None,
        system_prompt: None,
        codename: codename.map(str::to_string),
    };
    let mut f = std::fs::File::create(path).unwrap();
    writeln!(f, "{}", serde_json::to_string(&ev).unwrap()).unwrap();
}

fn append_line(path: &Path, v: &serde_json::Value) {
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap();
    writeln!(f, "{v}").unwrap();
}

#[test]
fn sub_agents_numbered_by_ulid_order_per_parent_and_role() {
    let tmp = tempfile::tempdir().unwrap();
    let store = RunStore::new(tmp.path().join("runs"));
    store
        .create(legacy_record(RUN, "legacy-fixture"), WF)
        .unwrap();
    // Step `alpha`'s agent ran as run_A; it dispatched `scout` twice (ULID
    // order sub_..01 then sub_..02, created out of order on disk) and the
    // first child dispatched a grandchild `scout`.
    append_line(
        &store.root.join(RUN).join("step_results.jsonl"),
        &serde_json::json!({
            "step_id": "alpha", "run_id": "run_A", "transcript_path": "/t/run_A.jsonl",
            "output": "", "success": true, "skipped": false, "rendered_prompt": "",
            "kind": "linear", "finished_at": "2026-09-10T19:34:35Z"
        }),
    );
    let s1 = "sub_01J9ZQ3K4M5N6P7Q8R9S0T1V01";
    let s2 = "sub_01J9ZQ3K4M5N6P7Q8R9S0T1V02";
    let s3 = "sub_01J9ZQ3K4M5N6P7Q8R9S0T1V03";
    let sub_t = |parent: &str, sub: &str| {
        store
            .root
            .join(parent)
            .join("sub")
            .join(sub)
            .join("transcript.jsonl")
    };
    write_run_start(&sub_t("run_A", s2), "scout", None);
    write_run_start(&sub_t("run_A", s1), "scout", None);
    write_run_start(&sub_t(s1, s3), "scout", None);

    // Expected: exactly what the dispatcher's `child_codename` mints, in
    // dispatch (ULID) order, against the run's post-walk namer.
    let m = minted();
    let alpha = m.step("alpha", "ag");
    let mut n = m.namer().with(|n| n.clone());
    let mut mint = |parent: &Codename| {
        let role = n.canonical_role("scout");
        let k = n.next_instance(parent, &role);
        parent.child(&role, Some(k))
    };
    let c1 = mint(&alpha);
    let c2 = mint(&alpha);
    let c3 = mint(&c1);
    assert!(c1.to_string().ends_with("#1") && c2.to_string().ends_with("#2"));
    assert!(c3.to_string().starts_with(&c1.to_string()));

    let mut d = LegacyNamer::open(&store, RUN);
    let subs = d.subs(&store).clone();
    assert_eq!(subs[s1].codename.as_deref(), Some(c1.to_string().as_str()));
    assert_eq!(subs[s2].codename.as_deref(), Some(c2.to_string().as_str()));
    assert_eq!(subs[s3].codename.as_deref(), Some(c3.to_string().as_str()));
    assert!(subs[s1].derived);
    assert_eq!(subs[s1].agent.as_deref(), Some("scout"));
    assert_eq!(subs[s1].provider.as_deref(), Some("anthropic"));
    assert_eq!(subs[s1].model.as_deref(), Some("claude-x"));

    // dispatch_started lacking a codename gets the derived one.
    let mut ev = serde_json::json!({"type": "dispatch_started", "run_id": RUN,
        "sub_run_id": s2, "agent": "scout", "transcript_path": "/x"});
    assert!(d.fill_event(&store, &mut ev));
    assert_eq!(ev["codename"], c2.to_string());
    assert_eq!(ev[DERIVED_KEY], true);
    // Never writes into the legacy run dir.
    assert!(!store.root.join(RUN).join("codenames.json").exists());
}

#[test]
fn stored_sub_codename_wins() {
    let tmp = tempfile::tempdir().unwrap();
    let store = RunStore::new(tmp.path().join("runs"));
    store
        .create(legacy_record(RUN, "legacy-fixture"), WF)
        .unwrap();
    let s1 = "sub_01J9ZQ3K4M5N6P7Q8R9S0T1V01";
    write_run_start(
        &store
            .root
            .join(RUN)
            .join("sub")
            .join(s1)
            .join("transcript.jsonl"),
        "scout",
        Some("jade-reef>kept#9"),
    );
    let mut d = LegacyNamer::open(&store, RUN);
    let s = &d.subs(&store)[s1];
    assert_eq!(s.codename.as_deref(), Some("jade-reef>kept#9"));
    assert!(!s.derived);
}

#[test]
fn fill_event_derives_missing_and_leaves_named_events_alone() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Arc::new(RunStore::new(tmp.path().join("runs")));
    store
        .create(legacy_record(RUN, "legacy-fixture"), WF)
        .unwrap();
    let m = minted();
    let mut namers = EventNamers::new(store.clone());

    let mut step = serde_json::json!({"type": "step_started", "run_id": RUN,
        "step_id": "beta", "kind": "linear", "agent": "ag"});
    assert!(namers.fill_row(RUN, &mut step));
    assert_eq!(step["codename"], m.step("beta", "ag").to_string());
    assert_eq!(step[DERIVED_KEY], true);

    let mut unit = serde_json::json!({"type": "unit_started", "run_id": RUN,
        "step_id": "fan", "index": 4, "unit_key": "e.rs", "agent": "triage",
        "transcript_path": "/t/run_u.jsonl"});
    assert!(namers.fill_row(RUN, &mut unit));
    assert_eq!(unit["codename"], m.unit("fan", "triage", 4).to_string());

    let mut agent = serde_json::json!({"type": "agent_started", "run_id": RUN,
        "step_id": "par", "unit_index": 1, "agent": "triage",
        "agent_run_id": "run_p", "transcript_path": "/t/run_p.jsonl"});
    assert!(namers.fill_row(RUN, &mut agent));
    assert_eq!(agent["codename"], m.sub("par", "p2", "triage").to_string());

    // Already-named and non-agent events: untouched.
    let named = serde_json::json!({"type": "step_started", "run_id": RUN,
        "step_id": "beta", "kind": "linear", "codename": "cobalt-harbor/heron"});
    let mut copy = named.clone();
    assert!(!namers.fill_row(RUN, &mut copy));
    assert_eq!(copy, named);
    let mut done = serde_json::json!({"type": "step_completed", "run_id": RUN,
        "step_id": "beta", "success": true, "duration_ms": 1});
    assert!(!namers.fill_row(RUN, &mut done));
    assert!(done.get("codename").is_none());

    // Typed path: a named event needs no re-serialization at all.
    let ev = Event::StepStarted {
        run_id: RUN.into(),
        step_id: "beta".into(),
        kind: rupu_orchestrator::runs::StepKind::Linear,
        agent: Some("ag".into()),
        host: None,
        codename: Some("cobalt-harbor/heron".into()),
    };
    assert!(namers.fill_typed(&ev).is_none());
    let ev = Event::StepStarted {
        run_id: RUN.into(),
        step_id: "beta".into(),
        kind: rupu_orchestrator::runs::StepKind::Linear,
        agent: Some("ag".into()),
        host: None,
        codename: None,
    };
    let row = namers.fill_typed(&ev).expect("derived");
    assert_eq!(row["codename"], m.step("beta", "ag").to_string());
    assert_eq!(row[DERIVED_KEY], true);
}

/// Read-only check against a REAL legacy run: point `RUPU_LEGACY_RUN_COPY`
/// at a COPY of a run dir (`<tmp>/runs/<run_id>`), never at `~/.rupu`.
#[test]
#[ignore]
fn print_real_legacy_run_names() {
    let Ok(dir) = std::env::var("RUPU_LEGACY_RUN_COPY") else {
        return;
    };
    let dir = PathBuf::from(dir);
    let run_id = dir.file_name().unwrap().to_str().unwrap().to_string();
    let store = RunStore::new(dir.parent().unwrap().to_path_buf());
    let store = Arc::new(store);
    let d = LegacyNamer::open(&store, &run_id);
    let mut steps: Vec<Value> = store
        .read_step_results(&run_id)
        .unwrap()
        .iter()
        .map(|r| serde_json::to_value(r).unwrap())
        .collect();
    d.fill_step_records(&mut steps);
    println!("crew: {}", rupu_codename::crew_for(&run_id));
    for s in &steps {
        println!(
            "step {} -> {} (derived={})",
            s["step_id"], s["codename"], s[DERIVED_KEY]
        );
        for i in s["items"].as_array().into_iter().flatten() {
            println!("  item {} -> {}", i["index"], i["codename"]);
        }
    }
    let mut namers = EventNamers::new(store.clone());
    for ev in read_events(&store, &run_id) {
        if let Some(row) = namers.fill_typed(&ev) {
            println!(
                "event {} {} -> {}",
                row["type"], row["step_id"], row["codename"]
            );
        }
    }
}

#[test]
fn event_namers_classify_once_per_run_and_never_open_a_namer_for_codename_era_runs() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Arc::new(RunStore::new(tmp.path().join("runs")));
    let modern_id = "run_01J9ZQ3K4M5N6P7Q8R9S0T1V2Y";
    let mut modern = legacy_record(modern_id, "legacy-fixture");
    modern.codename = Some("olive-pine".into());
    store.create(modern.clone(), WF).unwrap();
    store
        .create(legacy_record(RUN, "legacy-fixture"), WF)
        .unwrap();
    let mut namers = EventNamers::new(store.clone());

    // A codename-era run's codename-less agent step: never derived.
    let mut step = serde_json::json!({"type": "step_started", "run_id": modern_id,
        "step_id": "alpha", "kind": "linear", "agent": "ag"});
    assert!(!namers.fill_row(modern_id, &mut step));
    assert!(step.get("codename").is_none());
    assert!(namers.known_modern(modern_id));
    assert_eq!(
        namers.legacy_runs(),
        0,
        "no LegacyNamer for a codename-era run"
    );

    // note_record classifies without IO; legacy runs stay unclassified until
    // their first unnamed event.
    let mut other = EventNamers::new(store.clone());
    other.note_record(&modern);
    other.note_record(&legacy_record(RUN, "legacy-fixture"));
    assert!(other.known_modern(modern_id));
    assert!(!other.known_modern(RUN));
    assert_eq!(other.cached_runs(), 1);

    let mut legacy = serde_json::json!({"type": "step_started", "run_id": RUN,
        "step_id": "alpha", "kind": "linear", "agent": "ag"});
    assert!(namers.fill_row(RUN, &mut legacy));
    assert_eq!(namers.legacy_runs(), 1);
    assert_eq!(namers.cached_runs(), 2);

    namers.evict(RUN);
    namers.evict(modern_id);
    assert_eq!(namers.cached_runs(), 0);
}

#[test]
fn run_is_legacy_is_decided_by_the_run_record() {
    let mut r = legacy_record(RUN, "wf");
    assert!(run_is_legacy(&r));
    r.codename = Some(String::new());
    assert!(run_is_legacy(&r));
    r.codename = Some("jade-reef".into());
    assert!(!run_is_legacy(&r));
    // fill_detail_steps leaves a codename-era run's unnamed records alone.
    let tmp = tempfile::tempdir().unwrap();
    let store = RunStore::new(tmp.path().join("runs"));
    let mut detail = serde_json::json!({"steps": [
        {"step_id": "alpha", "kind": "linear", "run_id": "", "skipped": true}
    ]});
    fill_detail_steps(&store, &r, &mut detail);
    assert!(detail["steps"][0].get("codename").is_none());
}

#[test]
fn a_sub_run_miss_rebuilds_the_tree_at_most_once_per_refresh_window() {
    let tmp = tempfile::tempdir().unwrap();
    let store = RunStore::new(tmp.path().join("runs"));
    store
        .create(legacy_record(RUN, "legacy-fixture"), WF)
        .unwrap();
    let s1 = "sub_01J9ZQ3K4M5N6P7Q8R9S0T1V01";
    let s2 = "sub_01J9ZQ3K4M5N6P7Q8R9S0T1V02";
    let t = |sub: &str| {
        store
            .root
            .join(RUN)
            .join("sub")
            .join(sub)
            .join("transcript.jsonl")
    };
    write_run_start(&t(s1), "scout", None);
    let mut d = LegacyNamer::open(&store, RUN);
    assert!(d.sub(&store, s1).is_some());

    // Dispatched after the build: inside the window a miss does not rebuild.
    write_run_start(&t(s2), "scout", None);
    assert!(d.sub(&store, s2).is_none());
    // Once the window has passed, the next miss rebuilds and finds it.
    d.subs_built_at = Some(Instant::now() - SUB_REFRESH);
    assert!(d.sub(&store, s2).is_some());
}

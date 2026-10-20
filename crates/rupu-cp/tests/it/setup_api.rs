//! Plan 3a Package C over plain HTTP: `POST /api/sessions/:id/{stop,compact}`,
//! `POST /api/skills/{install,update,validate}`, `GET /api/fleet`, `POST
//! /api/fleet/install`, `POST /api/projects/init` and `GET
//! /api/engagement-profiles[/:id]` — the happy paths, each error status,
//! and `?host=<remote>` → 501 (every one of them is local-only).

// In-process test server client, not rupu egress (see sse.rs).
#![allow(clippy::disallowed_methods)]

use super::mcp_ops_setup::{http_get, spawn_setup, write_skill, Setup, BUSY, LIVE, SHELVED, WS};
use serde_json::{json, Value};

async fn post(s: &Setup, path: &str, body: Value) -> (u16, Value) {
    let r = reqwest::Client::new()
        .post(format!("http://{}{path}", s.addr))
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = r.status().as_u16();
    (status, r.json().await.unwrap_or(Value::Null))
}

// ---- sessions ----

#[tokio::test]
async fn stop_stops_a_live_session_and_refuses_the_rest() {
    let s = spawn_setup(true, true).await;
    let (st, v) = post(&s, &format!("/api/sessions/{LIVE}/stop"), json!({})).await;
    assert_eq!((st, v), (200, json!({"ok": true, "id": LIVE})));
    assert_eq!(*s.mutator.0.lock().unwrap(), vec![format!("stop {LIVE}")]);

    let missing = "ses_01J9ZQ3K4M5N6P7Q8R9S0T1NON";
    assert_eq!(
        post(&s, &format!("/api/sessions/{missing}/stop"), json!({}))
            .await
            .0,
        404
    );
    let (st, v) = post(&s, &format!("/api/sessions/{SHELVED}/stop"), json!({})).await;
    assert_eq!(st, 409, "{v}");
    assert!(v["error"].as_str().unwrap().contains("archived"), "{v}");
    let (st, v) = post(
        &s,
        &format!("/api/sessions/{LIVE}/stop?host=peer"),
        json!({}),
    )
    .await;
    assert_eq!(st, 501, "{v}");
    assert!(v["error"].as_str().unwrap().contains("local-only"), "{v}");
    // Nothing but the first stop reached the port.
    assert_eq!(s.mutator.0.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn compact_starts_detached_with_a_log_and_refuses_a_running_turn() {
    let s = spawn_setup(true, true).await;
    let (st, v) = post(
        &s,
        &format!("/api/sessions/{LIVE}/compact"),
        json!({"window": 200000}),
    )
    .await;
    assert_eq!(st, 202, "{v}");
    let log = s.global.join("sessions").join(LIVE).join("compact.log");
    let cid = v["compaction_id"].as_str().unwrap().to_string();
    assert!(cid.starts_with("cmp_"), "{v}");
    assert_eq!(
        v,
        json!({"ok": true, "id": LIVE, "compaction_id": cid, "log": log})
    );
    assert_eq!(
        *s.mutator.0.lock().unwrap(),
        vec![format!("compact {LIVE} Some(200000) {}", log.display())]
    );

    let (st, v) = post(&s, &format!("/api/sessions/{BUSY}/compact"), json!({})).await;
    assert_eq!(st, 409, "{v}");
    assert!(v["error"].as_str().unwrap().contains("running"), "{v}");
    assert_eq!(
        post(&s, &format!("/api/sessions/{SHELVED}/compact"), json!({}))
            .await
            .0,
        409
    );
    let missing = "ses_01J9ZQ3K4M5N6P7Q8R9S0T1NON";
    assert_eq!(
        post(&s, &format!("/api/sessions/{missing}/compact"), json!({}))
            .await
            .0,
        404
    );
    assert_eq!(
        post(
            &s,
            &format!("/api/sessions/{LIVE}/compact?host=peer"),
            json!({})
        )
        .await
        .0,
        501
    );
    assert_eq!(s.mutator.0.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_malformed_optional_body_is_refused_not_ignored() {
    let s = spawn_setup(true, true).await;
    for path in [
        format!("/api/sessions/{LIVE}/compact"),
        "/api/fleet/install".to_string(),
        "/api/skills/update".to_string(),
    ] {
        let r = reqwest::Client::new()
            .post(format!("http://{}{path}", s.addr))
            .header("content-type", "application/json")
            .body("{\"force\": tru")
            .send()
            .await
            .unwrap();
        assert_eq!(r.status().as_u16(), 400, "{path}");
    }
    assert!(s.mutator.0.lock().unwrap().is_empty());
    assert!(!s.global.join("agents").exists());
}

/// Mark `LIVE` as being compacted by `pid` (this test process: alive).
fn compacting(s: &Setup, pid: u32, start: Option<u64>) {
    rupu_runtime::services::session_compaction::begin(
        &s.global.join("sessions").join(LIVE),
        "cmp_01J9ZQ3K4M5N6P7Q8R9S0T1LIV",
        pid,
        start,
        rupu_launch::proc::process_alive,
    )
    .unwrap();
}

#[tokio::test]
async fn a_live_compaction_refuses_every_session_write_and_shows_in_the_detail() {
    let s = spawn_setup(true, true).await;
    compacting(&s, std::process::id(), rupu_launch::proc::own_start_stamp());
    let (_, v) = http_get(s.addr, &format!("/api/sessions/{LIVE}")).await;
    assert_eq!(v["compacting"], true, "{v}");
    assert_eq!(v["compaction"]["pid"], std::process::id(), "{v}");

    for (path, body) in [
        (
            format!("/api/sessions/{LIVE}/send"),
            json!({"prompt": "hi"}),
        ),
        (format!("/api/sessions/{LIVE}/stop"), json!({})),
        (format!("/api/sessions/{LIVE}/archive"), json!({})),
        (format!("/api/sessions/{LIVE}/compact"), json!({})),
    ] {
        let (st, v) = post(&s, &path, body).await;
        assert_eq!(st, 409, "{path}: {v}");
        assert!(
            v["error"].as_str().unwrap().contains("is being compacted"),
            "{v}"
        );
    }
    let r = reqwest::Client::new()
        .delete(format!("http://{}/api/sessions/{LIVE}", s.addr))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 409);
    // Nothing reached a port.
    assert!(s.mutator.0.lock().unwrap().is_empty());
    assert!(s.sender.0.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_stale_compaction_marker_refuses_nothing() {
    let s = spawn_setup(true, true).await;
    // pid 0 is never a live process: the marker is stale and cleared.
    compacting(&s, 0, None);
    let (_, v) = http_get(s.addr, &format!("/api/sessions/{LIVE}")).await;
    assert_eq!(v["compacting"], false, "{v}");
    assert_eq!(v["compaction"], Value::Null, "{v}");
    assert!(!s
        .global
        .join("sessions")
        .join(LIVE)
        .join("compaction.json")
        .exists());
    let (st, v) = post(
        &s,
        &format!("/api/sessions/{LIVE}/send"),
        json!({"prompt": "hi"}),
    )
    .await;
    assert_eq!(st, 200, "{v}");
    assert_eq!(
        post(&s, &format!("/api/sessions/{LIVE}/stop"), json!({}))
            .await
            .0,
        200
    );
    assert_eq!(*s.sender.0.lock().unwrap(), vec![format!("send {LIVE}")]);
}

#[tokio::test]
async fn a_finished_compactions_marker_does_not_refuse_the_next() {
    let s = spawn_setup(true, true).await;
    let dir = s.global.join("sessions").join(LIVE);
    // Two compactions back to back: the stub's child (pid 0) is gone at once,
    // so the second is not refused.
    for _ in 0..2 {
        let (st, v) = post(&s, &format!("/api/sessions/{LIVE}/compact"), json!({})).await;
        assert_eq!(st, 202, "{v}");
    }
    assert_eq!(
        rupu_runtime::services::session_compaction::live(&dir, rupu_launch::proc::process_alive)
            .unwrap(),
        None
    );
}

#[tokio::test]
async fn session_writes_need_cp_serve() {
    let s = spawn_setup(false, true).await;
    assert_eq!(
        post(&s, &format!("/api/sessions/{LIVE}/stop"), json!({}))
            .await
            .0,
        501
    );
    assert_eq!(
        post(&s, &format!("/api/sessions/{LIVE}/compact"), json!({}))
            .await
            .0,
        501
    );
    assert!(s.mutator.0.lock().unwrap().is_empty());
}

// ---- skills ----

#[tokio::test]
async fn install_from_a_local_directory_then_refuse_a_second_without_force() {
    let s = spawn_setup(true, true).await;
    let src = s.skill_src.display().to_string();
    let (st, v) = post(&s, "/api/skills/install", json!({"source": src})).await;
    assert_eq!(st, 200, "{v}");
    let installed = &v["installed"][0];
    assert_eq!(installed["name"], "demo-skill");
    assert_eq!(installed["layer"], "global");
    assert_eq!(installed["outcome"], "created");
    assert_eq!(installed["tracked"], true);
    assert_eq!(v["skipped"], json!([]));
    assert!(s.global.join("skills/demo-skill/SKILL.md").is_file());
    // The install is listed like any other.
    let (_, list) = http_get(s.addr, "/api/skills").await;
    assert_eq!(list["skills"][0]["name"], "demo-skill", "{list}");

    let (st, v) = post(&s, "/api/skills/install", json!({"source": src})).await;
    assert_eq!(st, 409, "{v}");
    let (st, v) = post(
        &s,
        "/api/skills/install",
        json!({"source": src, "force": true}),
    )
    .await;
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["installed"][0]["outcome"], "replaced");
}

#[tokio::test]
async fn install_into_a_project_lands_in_its_layer() {
    let s = spawn_setup(true, true).await;
    let src = s.skill_src.display().to_string();
    let (st, v) = post(
        &s,
        "/api/skills/install",
        json!({"source": src, "project": WS}),
    )
    .await;
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["installed"][0]["layer"], "project");
    // From a directory outside the project: not tracked for update.
    assert_eq!(v["installed"][0]["tracked"], false);
    assert!(s.project.join(".rupu/skills/demo-skill/SKILL.md").is_file());
    assert!(!s.global.join("skills/demo-skill").exists());
}

#[tokio::test]
async fn install_refusals_carry_their_status() {
    let s = spawn_setup(true, true).await;
    // A git source whose fetch fails.
    assert_eq!(
        post(&s, "/api/skills/install", json!({"source": "owner/repo"}))
            .await
            .0,
        502
    );
    // Not a directory, owner/repo or URL.
    assert_eq!(
        post(&s, "/api/skills/install", json!({"source": "::nope"}))
            .await
            .0,
        400
    );
    // `skills` and `all` together.
    let src = s.skill_src.display().to_string();
    let (st, _) = post(
        &s,
        "/api/skills/install",
        json!({"source": src, "all": true, "skills": ["demo-skill"]}),
    )
    .await;
    assert_eq!(st, 400);
    // A skill the source doesn't hold.
    let (st, v) = post(
        &s,
        "/api/skills/install",
        json!({"source": src, "skills": ["other"]}),
    )
    .await;
    assert_eq!(st, 400, "{v}");
    // An unknown project.
    let (st, _) = post(
        &s,
        "/api/skills/install",
        json!({"source": src, "project": "ws_01J9ZQ3K4M5N6P7Q8R9S0T1NON"}),
    )
    .await;
    assert_eq!(st, 404);
    // A remote host.
    assert_eq!(
        post(&s, "/api/skills/install?host=peer", json!({"source": src}))
            .await
            .0,
        501
    );
    assert!(!s.global.join("skills").exists());
}

#[tokio::test]
async fn skill_writes_need_the_installer_port() {
    let s = spawn_setup(true, false).await;
    let src = s.skill_src.display().to_string();
    assert_eq!(
        post(&s, "/api/skills/install", json!({"source": src}))
            .await
            .0,
        501
    );
    assert_eq!(post(&s, "/api/skills/update", json!({})).await.0, 501);
    let s = spawn_setup(false, true).await;
    assert_eq!(
        post(&s, "/api/skills/install", json!({"source": src}))
            .await
            .0,
        501
    );
}

#[tokio::test]
async fn update_reports_per_skill_outcomes() {
    let s = spawn_setup(true, true).await;
    let src = s.skill_src.display().to_string();
    assert_eq!(
        post(&s, "/api/skills/install", json!({"source": src}))
            .await
            .0,
        200
    );
    let (st, v) = post(&s, "/api/skills/update", json!({})).await;
    assert_eq!(st, 200, "{v}");
    assert_eq!(
        v,
        json!({"layer": "global", "results": [{"name": "demo-skill", "outcome": "up_to_date"}]})
    );
    // An edit to the source is picked up.
    write_skill(&s.skill_src, "demo-skill", "Demonstrates an update.");
    let (_, v) = post(&s, "/api/skills/update", json!({"names": ["demo-skill"]})).await;
    assert_eq!(v["results"][0]["outcome"], "updated", "{v}");
    let (_, v) = post(&s, "/api/skills/update", json!({"names": ["absent"]})).await;
    assert_ne!(v["results"][0]["outcome"], "updated", "{v}");
    assert_eq!(
        post(&s, "/api/skills/update?host=peer", json!({})).await.0,
        501
    );
}

#[tokio::test]
async fn validate_checks_a_directory_or_an_installed_skill() {
    let s = spawn_setup(true, true).await;
    let src = s.skill_src.display().to_string();
    let (st, v) = post(&s, "/api/skills/validate", json!({"path": src})).await;
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["ok"], true);
    assert_eq!(v["skills"][0]["name"], "demo-skill");
    assert_eq!(v["skills"][0]["status"], "ok");

    // A skill with no description is invalid.
    let bad = s.skill_src.parent().unwrap().join("bad-src");
    std::fs::create_dir_all(bad.join("undescribed")).unwrap();
    std::fs::write(
        bad.join("undescribed/SKILL.md"),
        "---\nname: undescribed\n---\nbody\n",
    )
    .unwrap();
    let (st, v) = post(
        &s,
        "/api/skills/validate",
        json!({"path": bad.display().to_string()}),
    )
    .await;
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["ok"], false, "{v}");
    assert_eq!(v["skills"][0]["status"], "invalid", "{v}");

    assert_eq!(
        post(&s, "/api/skills/validate", json!({"name": "demo-skill"}))
            .await
            .0,
        404
    );
    assert_eq!(
        post(&s, "/api/skills/install", json!({"source": src}))
            .await
            .0,
        200
    );
    let (st, v) = post(&s, "/api/skills/validate", json!({"name": "demo-skill"})).await;
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["skills"][0]["layer"], "global");
    assert_eq!(v["ok"], true);

    assert_eq!(post(&s, "/api/skills/validate", json!({})).await.0, 400);
    assert_eq!(
        post(&s, "/api/skills/validate", json!({"path": "relative"}))
            .await
            .0,
        400
    );
    assert_eq!(
        post(
            &s,
            "/api/skills/validate",
            json!({"name": "demo-skill", "layer": "project"})
        )
        .await
        .0,
        400
    );
    assert_eq!(
        post(&s, "/api/skills/validate?host=peer", json!({"path": src}))
            .await
            .0,
        501
    );
}

// ---- the stock fleet ----

#[tokio::test]
async fn fleet_lists_its_status_and_installs_into_either_root() {
    let s = spawn_setup(true, true).await;
    let (st, v) = http_get(s.addr, "/api/fleet").await;
    assert_eq!(st, 200, "{v}");
    let entries = v["entries"].as_array().unwrap();
    assert_eq!(entries.len(), rupu_templates::FLEET_MANIFEST.len());
    assert!(entries.iter().all(|e| e["global"] == "missing"), "{v}");
    assert!(entries.iter().all(|e| e.get("project").is_none()), "{v}");
    let recon = entries.iter().find(|e| e["name"] == "recon").unwrap();
    assert_eq!(recon["kind"], "agent");
    assert_eq!(recon["path"], "agents/recon.md");

    let (st, v) = post(&s, "/api/fleet/install", json!({})).await;
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["created"].as_u64().unwrap() as usize, entries.len());
    assert_eq!(v["root"], json!(s.global));
    std::fs::write(s.global.join("agents/recon.md"), "edited").unwrap();
    let (_, v) = post(&s, "/api/fleet/install", json!({})).await;
    assert_eq!(v["skipped"].as_u64().unwrap() as usize, entries.len());
    assert_eq!(
        std::fs::read_to_string(s.global.join("agents/recon.md")).unwrap(),
        "edited"
    );

    let (_, v) = http_get(s.addr, &format!("/api/fleet?ws_id={WS}")).await;
    let recon = v["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == "recon")
        .unwrap();
    assert_eq!(recon["global"], "modified", "{v}");
    assert_eq!(recon["project"], "missing", "{v}");

    let (st, v) = post(&s, "/api/fleet/install", json!({"project": WS})).await;
    assert_eq!(st, 200, "{v}");
    assert!(s.project.join(".rupu/agents/recon.md").is_file());
}

#[tokio::test]
async fn fleet_refusals_carry_their_status() {
    let s = spawn_setup(true, true).await;
    let unknown = "ws_01J9ZQ3K4M5N6P7Q8R9S0T1NON";
    assert_eq!(
        http_get(s.addr, &format!("/api/fleet?ws_id={unknown}"))
            .await
            .0,
        404
    );
    assert_eq!(
        post(&s, "/api/fleet/install", json!({"project": unknown}))
            .await
            .0,
        404
    );
    assert_eq!(http_get(s.addr, "/api/fleet?host=peer").await.0, 501);
    assert_eq!(
        post(&s, "/api/fleet/install?host=peer", json!({})).await.0,
        501
    );
    let ro = spawn_setup(false, true).await;
    assert_eq!(post(&ro, "/api/fleet/install", json!({})).await.0, 501);
    assert!(!s.global.join("agents").exists());
}

// ---- project init ----

#[tokio::test]
async fn init_bootstraps_a_directory_once() {
    let s = spawn_setup(true, true).await;
    let dir = s.global.parent().unwrap().join("new-project");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.display().to_string();
    let (st, v) = post(
        &s,
        "/api/projects/init",
        json!({"path": path, "with_samples": true}),
    )
    .await;
    assert_eq!(st, 200, "{v}");
    assert_eq!(
        v["files"][0],
        json!({"path": ".rupu/config.toml", "action": "created"})
    );
    assert_eq!(v["skipped"], 0);
    assert!(dir.join(".rupu/agents/review-diff.md").is_file());
    assert!(dir.join(".gitignore").is_file());

    let (st, v) = post(&s, "/api/projects/init", json!({"path": path})).await;
    assert_eq!(st, 409, "{v}");
    std::fs::write(dir.join(".rupu/agents/review-diff.md"), "edited").unwrap();
    let (st, v) = post(
        &s,
        "/api/projects/init",
        json!({"path": path, "with_samples": true, "force": true}),
    )
    .await;
    assert_eq!(st, 200, "{v}");
    assert_ne!(
        std::fs::read_to_string(dir.join(".rupu/agents/review-diff.md")).unwrap(),
        "edited"
    );
}

#[tokio::test]
async fn init_refusals_carry_their_status() {
    let s = spawn_setup(true, true).await;
    assert_eq!(
        post(&s, "/api/projects/init", json!({"path": "relative/dir"}))
            .await
            .0,
        400
    );
    let gone = s.global.parent().unwrap().join("not-there");
    assert_eq!(
        post(
            &s,
            "/api/projects/init",
            json!({"path": gone.display().to_string()})
        )
        .await
        .0,
        400
    );
    assert!(!gone.exists());
    let dir = s.project.display().to_string();
    assert_eq!(
        post(&s, "/api/projects/init?host=peer", json!({"path": dir}))
            .await
            .0,
        501
    );
    let ro = spawn_setup(false, true).await;
    assert_eq!(
        post(&ro, "/api/projects/init", json!({"path": dir}))
            .await
            .0,
        501
    );
}

// ---- engagement profiles ----

#[tokio::test]
async fn engagement_profiles_list_builtins_and_layered_overrides() {
    let s = spawn_setup(true, true).await;
    let (st, v) = http_get(s.addr, "/api/engagement-profiles").await;
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["errors"], json!([]));
    let network = v["profiles"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == "network")
        .unwrap()
        .clone();
    assert_eq!(network["layer"], "builtin");
    assert!(
        network["bundle"]["agents"]
            .as_array()
            .unwrap()
            .contains(&json!("recon")),
        "{network}"
    );
    assert!(!network["asset_kinds"].as_array().unwrap().is_empty());

    // A project profile overrides by id, and a new one appears.
    let dir = s.project.join(".rupu/profiles");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("lab.toml"),
        "id = \"lab\"\nname = \"Lab engagement\"\n",
    )
    .unwrap();
    let (_, v) = http_get(s.addr, &format!("/api/engagement-profiles/lab?ws_id={WS}")).await;
    assert_eq!(v["layer"], "project", "{v}");
    assert_eq!(v["name"], "Lab engagement");
    // Without the project, it isn't there.
    assert_eq!(
        http_get(s.addr, "/api/engagement-profiles/lab").await.0,
        404
    );
}

#[tokio::test]
async fn an_unloadable_profile_is_named_in_errors() {
    let s = spawn_setup(true, true).await;
    let dir = s.global.join("profiles");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("broken.toml"), "id = \"broken\"\nnot_a_key = 1\n").unwrap();
    let (st, v) = http_get(s.addr, "/api/engagement-profiles").await;
    assert_eq!(st, 200, "{v}");
    let errors = v["errors"].as_array().unwrap();
    assert_eq!(errors.len(), 1, "{v}");
    assert!(errors[0].as_str().unwrap().contains("broken.toml"), "{v}");
    let (_, v) = http_get(s.addr, "/api/engagement-profiles/code").await;
    assert_eq!(v["errors"].as_array().unwrap().len(), 1, "{v}");
    assert_eq!(
        http_get(s.addr, "/api/engagement-profiles?host=peer")
            .await
            .0,
        501
    );
    assert_eq!(
        http_get(s.addr, "/api/engagement-profiles/code?host=peer")
            .await
            .0,
        501
    );
}

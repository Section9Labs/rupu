//! The SCM-area operator tools: SCM accounts and rules, the tracked-repo
//! registry and auth status — and that no endpoint or tool of the area
//! ever answers with a stored secret.

// In-process test server client, not rupu egress (see sse.rs).
#![allow(clippy::disallowed_methods)]

use super::mcp::{assert_required_args, call_tool, json_of, rpc, spawn_operator_with, text_of};
use super::scm_api::{checkout, seed, send, SECRET};
use serde_json::{json, Value};

fn is_error(r: &Value) -> bool {
    r["isError"] == json!(true)
}

#[tokio::test]
async fn scm_tools_are_listed_with_their_required_arguments() {
    let (addr, _tmp) = spawn_operator_with(seed, None).await;
    let table: &[(&str, &[&str])] = &[
        ("scm_accounts.list", &[]),
        ("scm_rules.list", &[]),
        ("scm_rules.bind", &["account"]),
        ("repos.tracked", &[]),
        ("repos.attach", &["repo", "path"]),
        ("repos.prefer", &["repo", "path"]),
        ("repos.forget", &["id"]),
        ("auth.status", &[]),
    ];
    assert_required_args(addr, table).await;

    let list = rpc(addr, "tools/list", json!({})).await["result"]["tools"].clone();
    let hints = |name: &str| {
        list.as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == name)
            .unwrap()["annotations"]
            .clone()
    };
    for read in [
        "scm_accounts.list",
        "scm_rules.list",
        "repos.tracked",
        "auth.status",
    ] {
        assert_eq!(hints(read)["readOnlyHint"], json!(true), "{read}");
    }
    for destructive in ["scm_rules.bind", "repos.forget"] {
        assert_eq!(
            hints(destructive)["destructiveHint"],
            json!(true),
            "{destructive}"
        );
    }
    assert_ne!(hints("repos.attach")["destructiveHint"], json!(true));
}

#[tokio::test]
async fn scm_reads_answer_what_the_api_answers() {
    let (addr, tmp) = spawn_operator_with(seed, None).await;
    let r = call_tool(
        addr,
        "repos.attach",
        json!({"repo": "github:acme/widgets", "path": checkout(tmp.path(), "widgets")}),
    )
    .await;
    assert!(!is_error(&r), "{r}");
    for (tool, path) in [
        ("scm_accounts.list", "/api/scm/accounts"),
        ("scm_rules.list", "/api/scm/rules"),
        ("repos.tracked", "/api/repos/tracked"),
        ("auth.status", "/api/auth/status"),
    ] {
        let r = call_tool(addr, tool, json!({})).await;
        assert!(!is_error(&r), "{tool}: {r}");
        let (status, text) = send(addr, "GET", path, None).await;
        assert_eq!(status, 200, "{path}");
        let want: Value = serde_json::from_str(&text).unwrap();
        assert!(
            want.as_array().is_some_and(|a| !a.is_empty()),
            "{path}: {want}"
        );
        assert_eq!(json_of(&r), want, "{tool}");
    }
}

#[tokio::test]
async fn bind_attach_prefer_and_forget_record_through_mcp() {
    let (addr, tmp) = spawn_operator_with(seed, None).await;
    let r = call_tool(
        addr,
        "scm_rules.bind",
        json!({"owner": "MrBrutti/*", "account": "gh.work"}),
    )
    .await;
    assert!(!is_error(&r), "{r}");
    assert_eq!(
        r["structuredContent"]["rule"],
        json!({"owner": "MrBrutti/*", "account": "gh.work"})
    );
    assert!(r["structuredContent"].get("warning").is_none(), "{r}");
    let cfg: rupu_config::Config =
        toml::from_str(&std::fs::read_to_string(tmp.path().join("config.toml")).unwrap()).unwrap();
    assert_eq!(cfg.scm.rules[1].owner.as_deref(), Some("MrBrutti/*"));
    assert_eq!(cfg.scm.rules[1].account, "gh.work");

    let r = call_tool(addr, "scm_rules.bind", json!({"account": "gh.work"})).await;
    assert!(is_error(&r) && text_of(&r).starts_with("HTTP 400"), "{r}");

    let one = checkout(tmp.path(), "widgets");
    let two = checkout(tmp.path(), "widgets-2");
    let r = call_tool(
        addr,
        "repos.attach",
        json!({"repo": "github:acme/widgets", "path": one}),
    )
    .await;
    assert_eq!(json_of(&r)["preferred_path"], json!(one), "{r}");
    let r = call_tool(
        addr,
        "repos.prefer",
        json!({"repo": "github:acme/widgets", "path": two}),
    )
    .await;
    assert_eq!(json_of(&r)["preferred_path"], json!(two), "{r}");
    let on_disk: toml::Value = toml::from_str(
        &std::fs::read_to_string(tmp.path().join("repos/github--acme--widgets.toml")).unwrap(),
    )
    .unwrap();
    assert_eq!(on_disk["preferred_path"].as_str(), two.to_str());

    let r = call_tool(
        addr,
        "repos.forget",
        json!({"id": "github--acme--widgets", "path": two}),
    )
    .await;
    assert_eq!(json_of(&r)["remaining"]["known_paths"], json!([one]), "{r}");
    let r = call_tool(addr, "repos.forget", json!({"id": "github--acme--widgets"})).await;
    assert!(!is_error(&r), "{r}");
    assert!(!tmp.path().join("repos/github--acme--widgets.toml").exists());
    let r = call_tool(addr, "repos.forget", json!({"id": "github--acme--widgets"})).await;
    assert!(is_error(&r) && text_of(&r).starts_with("HTTP 404"), "{r}");
}

/// `repos.forget {id: "prefer"}` must not reach `/api/repos/tracked/prefer`.
#[tokio::test]
async fn a_forget_id_naming_the_prefer_route_is_refused() {
    let (addr, tmp) = spawn_operator_with(seed, None).await;
    let r = call_tool(
        addr,
        "repos.attach",
        json!({"repo": "github:acme/widgets", "path": checkout(tmp.path(), "widgets")}),
    )
    .await;
    assert!(!is_error(&r), "{r}");
    let r = call_tool(addr, "repos.forget", json!({"id": "prefer"})).await;
    assert!(is_error(&r), "{r}");
    assert!(tmp.path().join("repos/github--acme--widgets.toml").exists());
}

#[tokio::test]
async fn a_remote_host_is_refused_as_local_only() {
    let (addr, _tmp) = spawn_operator_with(seed, None).await;
    for tool in [
        "scm_accounts.list",
        "scm_rules.list",
        "repos.tracked",
        "auth.status",
    ] {
        let r = call_tool(addr, tool, json!({"host": "box"})).await;
        assert!(
            is_error(&r) && text_of(&r).starts_with("HTTP 501"),
            "{tool}: {r}"
        );
        assert!(text_of(&r).contains("local-only"), "{tool}: {r}");
    }
}

/// Every endpoint and tool of the area, on a home whose credential store,
/// and the origin urls of whose checkouts, hold [`SECRET`]: the secret is
/// in no answer, success or error.
#[tokio::test]
async fn no_answer_carries_a_stored_secret() {
    let (addr, tmp) = spawn_operator_with(seed, None).await;
    let one = checkout(tmp.path(), "widgets");
    let two = checkout(tmp.path(), "widgets-2");
    let corp = checkout(tmp.path(), "corp");
    let scp = checkout(tmp.path(), "scp");
    assert!(std::fs::read_to_string(tmp.path().join("auth.json"))
        .unwrap()
        .contains(SECRET));

    let mut answers: Vec<(String, String)> = Vec::new();
    let track = |p: &std::path::Path| json!({"repo": "github:acme/widgets", "path": p});
    let http: Vec<(&str, String, Option<Value>)> = vec![
        ("GET", "/api/scm/accounts".into(), None),
        ("GET", "/api/scm/rules".into(), None),
        (
            "POST",
            "/api/scm/rules".into(),
            Some(json!({"owner": "o/*", "account": "gh.work"})),
        ),
        (
            "POST",
            "/api/scm/rules".into(),
            Some(json!({"account": "x"})),
        ),
        ("POST", "/api/repos/tracked".into(), Some(track(&one))),
        (
            "POST",
            "/api/repos/tracked/prefer".into(),
            Some(track(&two)),
        ),
        (
            "POST",
            "/api/repos/tracked".into(),
            Some(json!({"repo": "github:acme/nope", "path": one})),
        ),
        // Remotes that parse to no repo, with the secret as user info.
        (
            "POST",
            "/api/repos/tracked".into(),
            Some(json!({"repo": "github:acme/w", "path": corp})),
        ),
        (
            "POST",
            "/api/repos/tracked/prefer".into(),
            Some(json!({"repo": "github:acme/w", "path": scp})),
        ),
        ("GET", "/api/repos/tracked".into(), None),
        ("GET", "/api/auth/status".into(), None),
        ("GET", "/api/auth/status?host=box".into(), None),
    ];
    for (method, path, body) in http {
        let (status, text) = send(addr, method, &path, body).await;
        answers.push((format!("HTTP {method} {path} -> {status}"), text));
    }
    // The calls reached the data: the registry now holds the origin url
    // with its embedded secret, which no answer may repeat.
    assert!(
        std::fs::read_to_string(tmp.path().join("repos/github--acme--widgets.toml"))
            .unwrap()
            .contains(SECRET)
    );
    for path in [
        format!(
            "/api/repos/tracked/github--acme--widgets?path={}",
            two.display()
        ),
        "/api/repos/tracked/github--acme--widgets".into(),
    ] {
        let (status, text) = send(addr, "DELETE", &path, None).await;
        answers.push((format!("HTTP DELETE {path} -> {status}"), text));
    }
    let tools: Vec<(&str, Value)> = vec![
        ("scm_accounts.list", json!({})),
        ("scm_rules.list", json!({})),
        (
            "scm_rules.bind",
            json!({"path": "~/p/*", "account": "gh.work"}),
        ),
        ("scm_rules.bind", json!({"account": "gh.work"})),
        ("repos.attach", track(&one)),
        ("repos.prefer", track(&two)),
        (
            "repos.attach",
            json!({"repo": "github:acme/nope", "path": two}),
        ),
        (
            "repos.attach",
            json!({"repo": "github:acme/w", "path": corp}),
        ),
        (
            "repos.prefer",
            json!({"repo": "github:acme/w", "path": scp}),
        ),
        ("repos.tracked", json!({})),
        ("auth.status", json!({})),
        ("auth.status", json!({"host": "box"})),
        (
            "repos.forget",
            json!({"id": "github--acme--widgets", "path": two}),
        ),
        ("repos.forget", json!({"id": "github--acme--widgets"})),
        ("repos.forget", json!({"id": "github--acme--widgets"})),
    ];
    for (tool, args) in tools {
        let r = rpc(addr, "tools/call", json!({"name": tool, "arguments": args})).await;
        answers.push((format!("tool {tool} {args}"), r.to_string()));
    }
    let listed = rpc(addr, "tools/list", json!({})).await;
    answers.push(("tools/list".into(), listed.to_string()));

    assert!(answers
        .iter()
        .any(|(_, a)| a.contains("github:acme/widgets")));
    // The unknown remotes were refused naming the url, user info dropped.
    assert!(answers
        .iter()
        .any(|(_, a)| a.contains("https://git.corp.example/acme/w.git")));
    assert!(answers
        .iter()
        .any(|(_, a)| a.contains("github.com:acme/w`")));
    for (what, answer) in &answers {
        assert!(!answer.contains(SECRET), "{what} leaked a secret: {answer}");
        assert!(!answer.contains("x-access-token"), "{what}: {answer}");
    }
}

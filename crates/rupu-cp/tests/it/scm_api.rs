//! `GET /api/scm/accounts`, `/api/scm/rules`, `/api/repos/tracked*` and
//! `/api/auth/status` over HTTP: the global config, the credential store
//! and the repo registry of a seeded `RUPU_HOME`.

// In-process test server client, not rupu egress (see sse.rs).
#![allow(clippy::disallowed_methods)]

use serde_json::{json, Value};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

/// Every secret the seed stores carries this marker.
pub(super) const SECRET: &str = "rupu-secret-7f3a9c";

/// A `RUPU_HOME` with an `[scm."gh.work"]` account and one rule, stored
/// credentials for several accounts (each holding [`SECRET`]) and two git
/// checkouts of `github:acme/widgets` whose origin url embeds [`SECRET`].
pub(super) fn seed(dir: &Path) {
    std::fs::write(
        dir.join("config.toml"),
        "# operator note\n\
         [scm.\"gh.work\"]\nkind = \"github\"\nbase_url = \"https://ghe.example\"\n\n\
         [[scm.rules]]\nowner = \"acme/*\"\naccount = \"gh.work\"\n",
    )
    .unwrap();
    let oauth = |expires_at: &str| {
        json!({
            "credentials": {
                "type": "oauth",
                "access": format!("access-{SECRET}"),
                "refresh": format!("refresh-{SECRET}"),
                "expires": 0,
            },
            "refresh_token": format!("stored-refresh-{SECRET}"),
            "expires_at": expires_at,
        })
        .to_string()
    };
    let auth = json!({
        "gh.work/api-key": format!("ghp-{SECRET}"),
        "anthropic/api-key": format!("sk-ant-{SECRET}"),
        "github/sso": oauth("2099-01-01T00:00:00Z"),
        "gitlab/sso": oauth("2000-01-01T00:00:00Z"),
    });
    std::fs::write(dir.join("auth.json"), auth.to_string()).unwrap();
    for name in ["widgets", "widgets-2"] {
        git_checkout(
            &dir.join("checkouts").join(name),
            &format!("https://x-access-token:{SECRET}@github.com/acme/widgets.git"),
        );
    }
    git_checkout(
        &dir.join("checkouts").join("other"),
        "git@github.com:acme/other.git",
    );
}

/// `git init` at `path` with `origin` set to `url`.
pub(super) fn git_checkout(path: &Path, url: &str) {
    std::fs::create_dir_all(path).unwrap();
    for args in [
        vec!["init", "-q", "-b", "main"],
        vec!["remote", "add", "origin", url],
    ] {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(path)
            .args(&args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
    }
}

/// A checkout's path as the registry records it.
pub(super) fn checkout(dir: &Path, name: &str) -> PathBuf {
    dir.join("checkouts").join(name).canonicalize().unwrap()
}

/// A CP over a seeded home; `writable` installs a launcher (`cp serve`).
pub(super) async fn spawn(writable: bool) -> (SocketAddr, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    seed(tmp.path());
    let mut state = rupu_cp::state::AppState::new(
        tmp.path().to_path_buf(),
        rupu_config::PricingConfig::default(),
    );
    if writable {
        state = state.with_launcher(Some(std::sync::Arc::new(super::mcp::NoopLauncher)));
    }
    let app = rupu_cp::server::router_with(
        state,
        None,
        rupu_cp::server::RouterOpts {
            web: false,
            mcp: None,
        },
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (addr, tmp)
}

/// `(status, body text)` of `method path` with an optional JSON body.
pub(super) async fn send(
    addr: SocketAddr,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, String) {
    let client = reqwest::Client::new();
    let url = format!("http://{addr}{path}");
    let req = match method {
        "GET" => client.get(url),
        "POST" => client.post(url),
        "DELETE" => client.delete(url),
        other => panic!("{other}"),
    };
    let req = match body {
        Some(b) => req.json(&b),
        None => req,
    };
    let r = req.send().await.unwrap();
    (r.status().as_u16(), r.text().await.unwrap())
}

async fn send_json(
    addr: SocketAddr,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let (status, text) = send(addr, method, path, body).await;
    (status, serde_json::from_str(&text).unwrap_or(Value::Null))
}

#[tokio::test]
async fn scm_accounts_list_kinds_rules_and_credential_state() {
    let (addr, _tmp) = spawn(false).await;
    let (status, body) = send_json(addr, "GET", "/api/scm/accounts", None).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        body,
        json!([
            {"account": "github", "kind": "github", "base_url": null, "rules": [],
             "credentials": true, "state": "ok"},
            {"account": "gitlab", "kind": "gitlab", "base_url": null, "rules": [],
             "credentials": true, "state": "needs_login"},
            {"account": "gh.work", "kind": "github", "base_url": "https://ghe.example",
             "rules": [{"owner": "acme/*"}], "credentials": true, "state": "ok"},
        ])
    );
}

#[tokio::test]
async fn auth_status_reports_modes_expiry_and_state() {
    let (addr, _tmp) = spawn(false).await;
    let (status, body) = send_json(addr, "GET", "/api/auth/status", None).await;
    assert_eq!(status, 200, "{body}");
    let row = |name: &str| {
        body.as_array()
            .unwrap()
            .iter()
            .find(|r| r["account"] == name)
            .unwrap_or_else(|| panic!("{name} missing: {body}"))
            .clone()
    };
    let names: Vec<&str> = body
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["account"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "anthropic",
            "openai",
            "gemini",
            "copilot",
            "github",
            "gitlab",
            "linear",
            "jira",
            "gh.work"
        ]
    );
    assert_eq!(
        row("gh.work"),
        json!({"account": "gh.work", "kind": "github", "api_key": true, "sso": null, "state": "ok"})
    );
    assert_eq!(
        row("github")["sso"],
        json!({"expires_at": "2099-01-01T00:00:00Z", "expired": false,
               "summary": row("github")["sso"]["summary"]})
    );
    assert!(row("github")["sso"]["summary"]
        .as_str()
        .unwrap()
        .starts_with("expires in "));
    assert_eq!(row("github")["state"], "ok");
    assert_eq!(row("gitlab")["sso"]["expired"], true);
    assert_eq!(row("gitlab")["sso"]["summary"], "expired — re-login");
    assert_eq!(row("gitlab")["state"], "needs_login");
    assert_eq!(row("anthropic")["kind"], "anthropic");
    assert_eq!(row("anthropic")["api_key"], true);
    assert_eq!(
        row("openai"),
        json!({"account": "openai", "kind": "openai", "api_key": false, "sso": null, "state": "absent"})
    );
    assert_eq!(row("linear")["kind"], Value::Null);
}

#[tokio::test]
async fn binding_a_rule_appends_it_to_the_global_config() {
    let (addr, tmp) = spawn(true).await;
    let (status, body) = send_json(addr, "GET", "/api/scm/rules", None).await;
    assert_eq!(status, 200);
    assert_eq!(body, json!([{"owner": "acme/*", "account": "gh.work"}]));

    let (status, body) = send_json(
        addr,
        "POST",
        "/api/scm/rules",
        Some(json!({"path": "~/Code/work/*", "account": "gh.work"})),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        body,
        json!({
            "rule": {"path": "~/Code/work/*", "account": "gh.work"},
            "rules": [
                {"owner": "acme/*", "account": "gh.work"},
                {"path": "~/Code/work/*", "account": "gh.work"},
            ],
        })
    );
    let text = std::fs::read_to_string(tmp.path().join("config.toml")).unwrap();
    assert!(text.contains("# operator note"), "{text}");
    let cfg: rupu_config::Config = toml::from_str(&text).unwrap();
    assert_eq!(
        cfg.scm.platforms["gh.work"].base_url.as_deref(),
        Some("https://ghe.example")
    );
    assert_eq!(cfg.scm.rules.len(), 2);

    // An undeclared account is written, with a warning.
    let (status, body) = send_json(
        addr,
        "POST",
        "/api/scm/rules",
        Some(json!({"owner": "x/*", "account": "gh-typo"})),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert!(
        body["warning"].as_str().unwrap().contains("gh-typo"),
        "{body}"
    );
    assert_eq!(body["rules"].as_array().unwrap().len(), 3);
}

#[tokio::test]
async fn a_bad_rule_is_400_and_writes_nothing() {
    let (addr, tmp) = spawn(true).await;
    let before = std::fs::read_to_string(tmp.path().join("config.toml")).unwrap();
    for body in [
        json!({"account": "gh.work"}),
        json!({"owner": "a/*", "path": "~/x", "account": "gh.work"}),
        json!({"owner": "a/*", "account": ""}),
    ] {
        let (status, reply) = send_json(addr, "POST", "/api/scm/rules", Some(body.clone())).await;
        assert_eq!(status, 400, "{body}: {reply}");
        assert!(reply["error"].is_string(), "{reply}");
    }
    // An unknown field is refused by the body itself.
    let (status, _) = send(
        addr,
        "POST",
        "/api/scm/rules",
        Some(json!({"owner": "a/*", "account": "gh.work", "extra": 1})),
    )
    .await;
    assert_eq!(status, 422);
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("config.toml")).unwrap(),
        before
    );
}

#[tokio::test]
async fn tracked_repos_attach_prefer_and_forget() {
    let (addr, tmp) = spawn(true).await;
    let one = checkout(tmp.path(), "widgets");
    let two = checkout(tmp.path(), "widgets-2");
    let (status, body) = send_json(addr, "GET", "/api/repos/tracked", None).await;
    assert_eq!((status, body), (200, json!([])));

    let (status, body) = send_json(
        addr,
        "POST",
        "/api/repos/tracked",
        Some(json!({"repo": "github:acme/widgets", "path": one})),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["id"], "github--acme--widgets");
    assert_eq!(body["repo"], "github:acme/widgets");
    assert_eq!(body["preferred_path"], json!(one));
    assert_eq!(body["known_paths"], json!([one]));
    assert_eq!(body["default_branch"], "main");

    let (status, body) = send_json(
        addr,
        "POST",
        "/api/repos/tracked/prefer",
        Some(json!({"repo": "github:acme/widgets", "path": two})),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["preferred_path"], json!(two));
    assert_eq!(body["known_paths"].as_array().unwrap().len(), 2);

    let (status, list) = send_json(addr, "GET", "/api/repos/tracked", None).await;
    assert_eq!(status, 200);
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert_eq!(list[0]["preferred_path"], json!(two));

    // Forget one path: the record stays with the other.
    let (status, body) = send_json(
        addr,
        "DELETE",
        &format!(
            "/api/repos/tracked/github--acme--widgets?path={}",
            two.display()
        ),
        None,
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["repo"], "github:acme/widgets");
    assert_eq!(body["remaining"]["known_paths"], json!([one]));
    assert_eq!(body["remaining"]["preferred_path"], json!(one));
    // A path that isn't tracked is a 404.
    let (status, _) = send(
        addr,
        "DELETE",
        &format!(
            "/api/repos/tracked/github--acme--widgets?path={}",
            two.display()
        ),
        None,
    )
    .await;
    assert_eq!(status, 404);

    // Forget the repo.
    let (status, body) = send_json(
        addr,
        "DELETE",
        "/api/repos/tracked/github--acme--widgets",
        None,
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        body,
        json!({"repo": "github:acme/widgets", "remaining": null})
    );
    assert!(!tmp.path().join("repos/github--acme--widgets.toml").exists());
    let (status, _) = send(
        addr,
        "DELETE",
        "/api/repos/tracked/github--acme--widgets",
        None,
    )
    .await;
    assert_eq!(status, 404);
}

#[tokio::test]
async fn attaching_a_path_that_is_not_the_repo_is_400() {
    let (addr, tmp) = spawn(true).await;
    let other = checkout(tmp.path(), "other");
    let cases = [
        json!({"repo": "github:acme/widgets", "path": other}),
        json!({"repo": "github:acme/widgets", "path": "checkouts/widgets"}),
        json!({"repo": "github:acme/widgets", "path": tmp.path().join("nope")}),
        json!({"repo": "github:acme/widgets", "path": tmp.path().join("repos-none")}),
        json!({"repo": "nonsense", "path": other}),
        json!({"repo": "linear:team/issues/4", "path": other}),
    ];
    for body in cases {
        let (status, reply) =
            send_json(addr, "POST", "/api/repos/tracked", Some(body.clone())).await;
        assert_eq!(status, 400, "{body}: {reply}");
    }
    let (status, reply) = send_json(
        addr,
        "POST",
        "/api/repos/tracked",
        Some(json!({"repo": "github:acme/widgets", "path": other})),
    )
    .await;
    assert_eq!(status, 400);
    assert!(
        reply["error"]
            .as_str()
            .unwrap()
            .contains("points at github:acme/other, not github:acme/widgets"),
        "{reply}"
    );
    assert!(!tmp.path().join("repos").exists());
}

#[tokio::test]
async fn writes_need_cp_serve() {
    let (addr, tmp) = spawn(false).await;
    let one = checkout(tmp.path(), "widgets");
    for (method, path, body) in [
        (
            "POST",
            "/api/scm/rules",
            Some(json!({"owner": "a/*", "account": "gh.work"})),
        ),
        (
            "POST",
            "/api/repos/tracked",
            Some(json!({"repo": "github:acme/widgets", "path": one})),
        ),
        (
            "POST",
            "/api/repos/tracked/prefer",
            Some(json!({"repo": "github:acme/widgets", "path": one})),
        ),
        ("DELETE", "/api/repos/tracked/github--acme--widgets", None),
    ] {
        let (status, text) = send(addr, method, path, body).await;
        assert_eq!(status, 501, "{method} {path}: {text}");
        assert!(text.contains("requires `rupu cp serve`"), "{text}");
    }
}

#[tokio::test]
async fn a_remote_host_is_501_local_only() {
    let (addr, tmp) = spawn(true).await;
    let one = checkout(tmp.path(), "widgets");
    let track = json!({"repo": "github:acme/widgets", "path": one});
    for (method, path, body) in [
        ("GET", "/api/scm/accounts?host=box", None),
        ("GET", "/api/scm/rules?host=box", None),
        (
            "POST",
            "/api/scm/rules?host=box",
            Some(json!({"owner": "a/*", "account": "gh.work"})),
        ),
        ("GET", "/api/repos/tracked?host=box", None),
        ("POST", "/api/repos/tracked?host=box", Some(track.clone())),
        ("POST", "/api/repos/tracked/prefer?host=box", Some(track)),
        (
            "DELETE",
            "/api/repos/tracked/github--acme--widgets?host=box",
            None,
        ),
        ("GET", "/api/auth/status?host=box", None),
    ] {
        let (status, text) = send(addr, method, path, body).await;
        assert_eq!(status, 501, "{method} {path}: {text}");
        assert!(text.contains("local-only"), "{text}");
    }
    // `host=local` is this host.
    let (status, _) = send(addr, "GET", "/api/auth/status?host=local", None).await;
    assert_eq!(status, 200);
    assert!(!tmp.path().join("repos").exists());
}

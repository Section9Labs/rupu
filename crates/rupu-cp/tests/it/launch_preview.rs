//! `POST /api/launch/preview`: the customer and the provider / fallback /
//! SCM accounts a launch from a directory would use, resolved the way the
//! launch resolves them.

// Throwaway in-process mock-server client, not rupu's egress
// (choke_point.rs's guard test already exempts everything under `/tests/`
// on that basis).
#![allow(clippy::disallowed_methods)]

use std::path::{Path, PathBuf};

use reqwest::StatusCode;
use rupu_workspace::{CustomerStore, NewCustomer, ProjectRef};
use serde_json::{json, Value};

async fn spawn(global: &Path) -> String {
    let state =
        rupu_cp::state::AppState::new(global.to_path_buf(), rupu_config::PricingConfig::default());
    let app = rupu_cp::server::router(state, None);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

fn git(dir: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {out:?}");
}

struct Fixture {
    _tmp: tempfile::TempDir,
    global: PathBuf,
    project: PathBuf,
}

/// A project (registered as `ws_acme`, assigned to `acme`) holding one
/// workflow `w` with a single step on agent `echo` (no `provider:`), a git
/// `origin` of `acme-corp/web`, and global SCM rules.
fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path().join("global");
    let project = tmp.path().join("web");
    std::fs::create_dir_all(global.join("workspaces")).unwrap();
    std::fs::create_dir_all(project.join(".rupu/workflows")).unwrap();
    std::fs::create_dir_all(project.join(".rupu/agents")).unwrap();
    let project = project.canonicalize().unwrap();
    let global = global.canonicalize().unwrap();

    std::fs::write(
        project.join(".rupu/workflows/w.yaml"),
        "name: w\nsteps:\n  - id: one\n    agent: echo\n    prompt: hi\n",
    )
    .unwrap();
    std::fs::write(
        project.join(".rupu/agents/echo.md"),
        "---\nname: echo\ndescription: echoes\n---\nEcho.\n",
    )
    .unwrap();
    std::fs::write(
        project.join(".rupu/agents/pinned.md"),
        "---\nname: pinned\nprovider: openai\n---\nPinned.\n",
    )
    .unwrap();

    git(&project, &["init", "-q"]);
    git(
        &project,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/acme-corp/web.git",
        ],
    );

    std::fs::write(
        global.join("workspaces/ws_acme.toml"),
        format!(
            "id = \"ws_acme\"\npath = \"{}\"\ncreated_at = \"2026-01-01T00:00:00Z\"\n",
            project.display()
        ),
    )
    .unwrap();
    std::fs::write(
        global.join("config.toml"),
        "[scm.github-acme]\nkind = \"github\"\n\n\
         [[scm.rules]]\nowner = \"acme-corp\"\naccount = \"github-acme\"\n",
    )
    .unwrap();

    let store = CustomerStore::new(&global);
    store
        .create(
            "acme",
            &NewCustomer {
                name: "Acme".into(),
                ..Default::default()
            },
        )
        .unwrap();
    store.assign("acme", ProjectRef::Id("ws_acme")).unwrap();
    std::fs::write(
        store.config_path("acme"),
        "default_provider = \"anthropic-acme\"\n\n\
         [[recovery.fallbacks]]\nprovider = \"openai\"\nmodel = \"gpt-x\"\n",
    )
    .unwrap();

    Fixture {
        _tmp: tmp,
        global,
        project,
    }
}

/// An api-key credential for `account` in the test home's auth file, so the
/// account counts as registered (a connector builds only with credentials).
async fn store_credential(global: &Path, account: &str) {
    rupu_auth::KeychainResolver::at(global.join("auth.json"))
        .store_named(
            account,
            rupu_providers::AuthMode::ApiKey,
            &rupu_auth::StoredCredential::api_key("test-key"),
        )
        .await
        .unwrap();
}

async fn preview_w(base: &str, f: &Fixture) -> Value {
    let resp = post(
        base,
        json!({ "workflow": "w", "working_dir": f.project.display().to_string() }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    resp.json().await.unwrap()
}

async fn post(base: &str, body: Value) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("{base}/api/launch/preview"))
        .json(&body)
        .send()
        .await
        .unwrap()
}

fn account<'a>(body: &'a Value, role: &str, account: &str) -> &'a Value {
    body["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["role"] == role && e["account"] == account)
        .unwrap_or_else(|| panic!("no {role} account {account} in {body}"))
}

#[tokio::test]
async fn previews_the_customer_provider_fallback_and_scm_accounts() {
    let f = fixture();
    store_credential(&f.global, "github-acme").await;
    let base = spawn(&f.global).await;
    let resp = post(
        &base,
        json!({ "workflow": "w", "working_dir": f.project.display().to_string() }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = resp.json().await.unwrap();

    assert_eq!(body["customer"]["slug"], "acme");
    assert_eq!(body["customer"]["name"], "Acme");

    let provider = account(&body, "provider", "anthropic-acme");
    assert_eq!(provider["source"], "customer default");
    assert_eq!(provider["agents"], json!(["echo"]));

    let fallback = account(&body, "fallback", "openai");
    assert_eq!(fallback["source"], "customer [recovery].fallbacks");
    assert_eq!(fallback["kind"], "openai");

    let scm = account(&body, "scm", "github-acme");
    assert_eq!(scm["source"], "rule owner = acme-corp");
    assert_eq!(scm["kind"], "github");
    assert_eq!(body["warnings"], json!([]));
}

#[tokio::test]
async fn previews_a_single_agent_and_attributes_a_pinned_provider() {
    let f = fixture();
    let base = spawn(&f.global).await;
    let resp = post(
        &base,
        json!({ "agent": "pinned", "working_dir": f.project.display().to_string() }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(
        account(&body, "provider", "openai")["source"],
        "agent frontmatter"
    );

    let resp = post(
        &base,
        json!({ "agent": "nope", "working_dir": f.project.display().to_string() }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn an_unknown_agent_in_a_workflow_is_a_warning_not_an_error() {
    let f = fixture();
    store_credential(&f.global, "github-acme").await;
    std::fs::write(
        f.project.join(".rupu/workflows/ghosty.yaml"),
        "name: ghosty\nsteps:\n  - id: one\n    agent: ghost\n    prompt: hi\n",
    )
    .unwrap();
    let base = spawn(&f.global).await;
    let resp = post(
        &base,
        json!({ "workflow": "ghosty", "working_dir": f.project.display().to_string() }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = resp.json().await.unwrap();
    let warnings = body["warnings"].as_array().unwrap();
    assert!(
        warnings
            .iter()
            .any(|w| w.as_str().unwrap().contains("ghost")),
        "{body}"
    );
    // The SCM account still resolves.
    account(&body, "scm", "github-acme");
}

#[tokio::test]
async fn a_dangling_assignment_is_a_409_the_launch_would_hit_too() {
    let f = fixture();
    std::fs::remove_dir_all(f.global.join("customers/acme")).unwrap();
    let base = spawn(&f.global).await;
    let resp = post(
        &base,
        json!({ "workflow": "w", "working_dir": f.project.display().to_string() }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let body: Value = resp.json().await.unwrap();
    assert!(body["error"].as_str().unwrap().contains("acme"), "{body}");
}

#[tokio::test]
async fn a_dir_without_a_customer_previews_without_one() {
    let f = fixture();
    CustomerStore::new(&f.global)
        .unassign(ProjectRef::Id("ws_acme"))
        .unwrap();
    let base = spawn(&f.global).await;
    let resp = post(
        &base,
        json!({ "workflow": "w", "working_dir": f.project.display().to_string() }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = resp.json().await.unwrap();
    assert!(body["customer"].is_null(), "{body}");
    // No customer layer: the built-in default provider.
    account(&body, "provider", "anthropic");
}

#[tokio::test]
async fn the_body_must_name_exactly_one_target() {
    let f = fixture();
    let base = spawn(&f.global).await;
    for body in [
        json!({}),
        json!({ "workflow": "w", "agent": "echo" }),
        json!({ "workflow": "w", "working_dir": "/x", "scope_kind": "global" }),
    ] {
        let resp = post(&base, body.clone()).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "{body}");
    }
    let resp = post(
        &base,
        json!({ "workflow": "missing", "working_dir": f.project.display().to_string() }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn several_credentialed_accounts_and_no_matching_rule_is_a_warning() {
    let f = fixture();
    store_credential(&f.global, "github-acme").await;
    store_credential(&f.global, "github-other").await;
    std::fs::write(
        f.global.join("config.toml"),
        "[scm.github-acme]\nkind = \"github\"\n\n[scm.github-other]\nkind = \"github\"\n",
    )
    .unwrap();
    let base = spawn(&f.global).await;
    let body = preview_w(&base, &f).await;
    assert!(
        body["accounts"]
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["role"] != "scm"),
        "{body}"
    );
    assert!(
        body["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w.as_str().unwrap().contains("no [[scm.rules]]")),
        "{body}"
    );
}

/// An account declared without a credential is not registered at run time,
/// so it neither makes the choice ambiguous nor gets picked.
#[tokio::test]
async fn an_uncredentialed_declared_account_is_not_a_candidate() {
    let f = fixture();
    store_credential(&f.global, "github-acme").await;
    std::fs::write(
        f.global.join("config.toml"),
        "[scm.github-acme]\nkind = \"github\"\n\n[scm.github-other]\nkind = \"github\"\n",
    )
    .unwrap();
    let base = spawn(&f.global).await;
    let body = preview_w(&base, &f).await;
    assert_eq!(
        account(&body, "scm", "github-acme")["source"],
        "only github account"
    );
    assert_eq!(body["warnings"], json!([]));
}

#[tokio::test]
async fn a_rule_naming_an_uncredentialed_account_is_a_warning() {
    let f = fixture(); // rule owner = acme-corp -> github-acme, no credential
    let base = spawn(&f.global).await;
    let body = preview_w(&base, &f).await;
    // No credentialed github account at all.
    assert!(
        body["accounts"]
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["role"] != "scm"),
        "{body}"
    );
    assert!(
        body["warnings"].as_array().unwrap().iter().any(|w| w
            .as_str()
            .unwrap()
            .contains("no github account has stored credentials")),
        "{body}"
    );
}

#[tokio::test]
async fn an_unreadable_credential_store_keeps_the_account_and_says_so() {
    let f = fixture();
    std::fs::write(f.global.join("auth.json"), "{ not json").unwrap();
    let base = spawn(&f.global).await;
    let body = preview_w(&base, &f).await;
    assert_eq!(
        account(&body, "scm", "github-acme")["source"],
        "rule owner = acme-corp · credentials not checked"
    );
    assert!(
        body["warnings"].as_array().unwrap().iter().any(|w| w
            .as_str()
            .unwrap()
            .contains("credential store could not be read")),
        "{body}"
    );
}

#[tokio::test]
async fn an_origin_off_github_and_gitlab_com_is_a_warning() {
    let f = fixture();
    git(
        &f.project,
        &[
            "remote",
            "set-url",
            "origin",
            "https://git.example.com/acme/web.git",
        ],
    );
    let base = spawn(&f.global).await;
    let body = preview_w(&base, &f).await;
    assert!(
        body["accounts"]
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["role"] != "scm"),
        "{body}"
    );
    let warnings = body["warnings"].as_array().unwrap();
    assert!(
        warnings.iter().any(|w| {
            let w = w.as_str().unwrap();
            w.contains("https://git.example.com/acme/web.git") && w.contains("can't be previewed")
        }),
        "{body}"
    );
}

/// One malformed agent file fails the loader for every agent; that is ONE
/// warning, not one per agent.
#[tokio::test]
async fn a_malformed_agent_file_warns_once() {
    let f = fixture();
    std::fs::write(f.project.join(".rupu/agents/broken.md"), "no frontmatter\n").unwrap();
    std::fs::write(
        f.project.join(".rupu/workflows/two.yaml"),
        "name: two\nsteps:\n  - id: a\n    agent: echo\n    prompt: x\n  - id: b\n    agent: pinned\n    prompt: y\n",
    )
    .unwrap();
    let base = spawn(&f.global).await;
    let resp = post(
        &base,
        json!({ "workflow": "two", "working_dir": f.project.display().to_string() }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = resp.json().await.unwrap();
    let loads = body["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|w| w.as_str().unwrap().contains("could not be loaded"))
        .count();
    assert_eq!(loads, 1, "{body}");
}

#[tokio::test]
async fn a_project_scope_selects_the_launch_directory() {
    let f = fixture();
    store_credential(&f.global, "github-acme").await;
    let base = spawn(&f.global).await;
    let resp = post(
        &base,
        json!({ "workflow": "w", "scope_kind": "project", "scope_id": "ws_acme" }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["customer"]["slug"], "acme");
    account(&body, "scm", "github-acme");
}

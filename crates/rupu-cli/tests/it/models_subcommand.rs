use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn models_list_prints_table_header() {
    let dir = tempfile::tempdir().unwrap();
    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", dir.path())
        .env(
            "RUPU_CACHE_DIR_OVERRIDE",
            dir.path().join("cache/models").to_str().unwrap(),
        )
        .args(["models", "list", "--provider", "copilot"])
        .assert()
        .success()
        .stdout(predicate::str::contains("PROVIDER"))
        .stdout(predicate::str::contains("MODEL"))
        .stdout(predicate::str::contains("SOURCE"));
}

#[test]
fn models_list_copilot_shows_baked_in_entries_offline() {
    let dir = tempfile::tempdir().unwrap();
    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", dir.path())
        .env(
            "RUPU_CACHE_DIR_OVERRIDE",
            dir.path().join("cache/models").to_str().unwrap(),
        )
        .args(["models", "list", "--provider", "copilot"])
        .assert()
        .success()
        .stdout(predicate::str::contains("gpt-4o"))
        .stdout(predicate::str::contains("baked-in"));
}

// ── Non-builtin (multi-account) providers ────────────────────────────────────
//
// `rupu models refresh|list --provider <account>` used to filter against a
// hardcoded four-name builtin array, so a declared account never matched any
// loop iteration: refresh exited 0 having done nothing at all, and list
// printed an empty table. Both must now resolve the name the same way `run`
// does — through `provider_factory::is_dispatchable_provider`/`resolve_kind`.

/// Write a global `config.toml` under `home`.
fn write_cfg(home: &std::path::Path, body: &str) {
    std::fs::create_dir_all(home).unwrap();
    std::fs::write(home.join("config.toml"), body).unwrap();
}

fn models_cmd(home: &std::path::Path) -> Command {
    let mut c = Command::cargo_bin("rupu").unwrap();
    c.env("RUPU_HOME", home).env(
        "RUPU_CACHE_DIR_OVERRIDE",
        home.join("cache/models").to_str().unwrap(),
    );
    c
}

/// The headline bug: a declared account is a no-op that exits 0 in silence.
/// With no credential for it the refresh must still *fail loudly for that
/// account* — which proves it entered the loop rather than being filtered
/// out before anything was attempted — and, being the only target, fail the
/// command.
#[test]
fn models_refresh_named_account_is_not_a_silent_no_op() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    write_cfg(home, "[providers.anthropic-work]\nkind = \"anthropic\"\n");
    models_cmd(home)
        // Hermetic even if the developer's shell exports a real key: the
        // listing can only reach a closed local port, never the vendor.
        .env(
            "RUPU_ANTHROPIC_BASE_URL_OVERRIDE",
            "http://127.0.0.1:9/v1/messages",
        )
        .args(["models", "refresh", "--provider", "anthropic-work"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("anthropic-work"));
}

/// The positive case, end to end through the real binary: a declared
/// `kind = "anthropic"` account must build an **Anthropic** client and
/// authenticate with **that account's** credential. The mock serves
/// Anthropic's `GET /v1/models` shape; hitting it at all proves the vendor
/// dispatch, and the `x-api-key` match proves the account's own key was used.
#[tokio::test(flavor = "multi_thread")]
async fn models_refresh_named_account_reaches_its_vendor_with_its_own_credential() {
    let server = httpmock::MockServer::start_async().await;
    let mock = server.mock(|when, then| {
        when.method(httpmock::Method::GET)
            .path("/v1/models")
            .header("x-api-key", "sk-account-scoped-key");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(serde_json::json!({
                "data": [{"id": "claude-mythos-5"}, {"id": "claude-sonnet-4-6"}]
            }));
    });

    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    write_cfg(home, "[providers.anthropic-work]\nkind = \"anthropic\"\n");
    models_cmd(home)
        // `models_request` strips `/v1/messages` off the base and appends
        // `/v1/models` — same seam `provider_factory` already honours.
        .env(
            "RUPU_ANTHROPIC_BASE_URL_OVERRIDE",
            format!("{}/v1/messages", server.url("")),
        )
        // `env_api_key` mangles `anthropic-work` -> `ANTHROPIC_WORK`.
        .env("RUPU_ANTHROPIC_WORK_API_KEY", "sk-account-scoped-key")
        .args(["models", "refresh", "--provider", "anthropic-work"])
        .assert()
        .success()
        .stdout(predicate::str::contains("anthropic-work"))
        .stdout(predicate::str::contains("2 models"));

    mock.assert_hits(1);

    // ... and the refreshed list is then visible through `models list`,
    // which has to load that account's cache to show it.
    models_cmd(home)
        .args(["models", "list", "--provider", "anthropic-work"])
        .assert()
        .success()
        .stdout(predicate::str::contains("claude-mythos-5"))
        .stdout(predicate::str::contains("live"));
}

/// A typo must be a real error with a non-zero exit, naming what it tried.
/// Silent success on a command that did nothing is the failure mode here.
#[test]
fn models_refresh_unresolvable_provider_errors_non_zero() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    std::fs::create_dir_all(home).unwrap();
    models_cmd(home)
        .args(["models", "refresh", "--provider", "anthropc"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("anthropc"))
        .stderr(predicate::str::contains("rupu auth login"));
}

#[test]
fn models_list_unresolvable_provider_errors_non_zero() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    std::fs::create_dir_all(home).unwrap();
    models_cmd(home)
        .args(["models", "list", "--provider", "anthropc"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("anthropc"));
}

/// `docs/providers/openai-compatible.md` documents exactly this command and
/// says the declared models "surface in `rupu models list --provider oracle`".
/// They did not: `list` only ever iterated the four builtin names, so the
/// custom entries `build_registry` had already loaded were never read back.
#[test]
fn models_list_surfaces_a_declared_openai_compatible_accounts_models() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    write_cfg(
        home,
        "[providers.oracle]\nkind = \"openai-compatible\"\n\
         base_url = \"http://127.0.0.1:9\"\ndefault_model = \"glm\"\n\
         [[providers.oracle.models]]\nid = \"glm\"\ncontext_window = 4096\n",
    );
    models_cmd(home)
        .args(["models", "list", "--provider", "oracle"])
        .assert()
        .success()
        .stdout(predicate::str::contains("glm"))
        .stdout(predicate::str::contains("custom"));
}

/// An openai-compatible account with a vLLM-style `GET /v1/models` endpoint
/// is refreshable like any other vendor: the listing's `max_model_len` becomes
/// the model's input limit and is then visible through `models list`.
#[test]
fn models_refresh_openai_compatible_account_fetches_its_v1_models() {
    let server = httpmock::MockServer::start();
    let m = server.mock(|when, then| {
        when.method(httpmock::Method::GET).path("/v1/models");
        then.status(200).json_body(serde_json::json!({
            "data": [{ "id": "box-model", "max_model_len": 65536 }]
        }));
    });
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    write_cfg(
        home,
        &format!(
            "[providers.boxy]\nkind = \"openai-compatible\"\nbase_url = \"{}\"\n",
            server.url("")
        ),
    );
    models_cmd(home)
        // An openai-compatible account still authenticates: the factory
        // resolves a credential before it builds the client.
        .env("RUPU_BOXY_API_KEY", "sk-boxy-test-key")
        .args(["models", "refresh", "--provider", "boxy"])
        .assert()
        .success()
        .stdout(predicate::str::contains("refreshed boxy (1 models)"));
    m.assert();
    // Table: the row is live, carries its limit, and shows a relative age.
    models_cmd(home)
        .args(["models", "list", "--provider", "boxy"])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("65536")
                .and(predicate::str::contains("live"))
                .and(predicate::str::contains("just now")),
        );
    // JSON: a live row carries a real RFC 3339 `fetched_at`, never null.
    let out = models_cmd(home)
        .args(["models", "list", "--provider", "boxy", "--format", "json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    let row = &v["rows"][0];
    assert_eq!(row["source"], "live", "{row}");
    assert_eq!(row["context"], 65536, "{row}");
    let fetched = row["fetched_at"].as_str().expect("live row has fetched_at");
    chrono::DateTime::parse_from_rfc3339(fetched).expect("fetched_at is RFC 3339");
}

/// An openai-compatible account whose endpoint serves no model list (the
/// vendor is real, the route is not) reports that failure against the
/// account instead of claiming a refresh of nothing — and, as the only
/// target, exits non-zero.
#[test]
fn models_refresh_openai_compatible_account_reports_a_missing_listing() {
    let server = httpmock::MockServer::start();
    server.mock(|when, then| {
        when.method(httpmock::Method::GET).path("/v1/models");
        then.status(404);
    });
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    write_cfg(
        home,
        &format!(
            "[providers.boxy]\nkind = \"openai-compatible\"\nbase_url = \"{}\"\n",
            server.url("")
        ),
    );
    models_cmd(home)
        .env("RUPU_BOXY_API_KEY", "sk-boxy-test-key")
        .args(["models", "refresh", "--provider", "boxy"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("skip boxy"))
        .stderr(predicate::str::contains("404"));
}

/// `list` surfaces both limits and when the live list was fetched, even with
/// nothing cached (the table header is the contract).
#[test]
fn models_list_shows_output_and_fetched_columns() {
    let dir = tempfile::tempdir().unwrap();
    models_cmd(dir.path())
        .args(["models", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("OUTPUT").and(predicate::str::contains("FETCHED")));
}

/// A config-declared row was never fetched: its FETCHED cell is `-` and its
/// JSON `fetched_at` is null, even though it sits beside live rows' timestamps.
#[test]
fn models_list_custom_row_has_no_fetched_at() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    write_cfg(
        home,
        "[providers.oracle]\nkind = \"openai-compatible\"\n\
         base_url = \"http://127.0.0.1:9\"\ndefault_model = \"glm\"\n\
         [[providers.oracle.models]]\nid = \"glm\"\ncontext_window = 4096\n",
    );
    let out = models_cmd(home)
        .args(["models", "list", "--provider", "oracle", "--format", "json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    let row = &v["rows"][0];
    assert_eq!(row["source"], "custom", "{row}");
    assert_eq!(row["context"], 4096, "{row}");
    assert!(row["output"].is_null(), "{row}");
    assert!(row["fetched_at"].is_null(), "{row}");
}

/// CSV is a stable contract: the header is exactly these six columns, in order.
#[test]
fn models_list_csv_header_is_exactly_the_documented_columns() {
    let dir = tempfile::tempdir().unwrap();
    let out = models_cmd(dir.path())
        .args(["models", "list", "--provider", "copilot", "--format", "csv"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8(out).unwrap();
    assert_eq!(
        text.lines().next(),
        Some("provider,model,source,context,output,fetched_at"),
        "{text}"
    );
}

/// The JSON report carries the new fields too (version 2): an absent limit is
/// `null`, not `0`, and `fetched_at` is only set for rows that came from a
/// live fetch.
#[test]
fn models_list_json_carries_output_and_fetched_at() {
    let dir = tempfile::tempdir().unwrap();
    let out = models_cmd(dir.path())
        .args([
            "models",
            "list",
            "--provider",
            "copilot",
            "--format",
            "json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(v["version"], 2);
    let row = &v["rows"][0];
    assert!(row.get("output").is_some(), "{row}");
    assert!(row.get("fetched_at").is_some(), "{row}");
    assert!(
        row["output"].is_null(),
        "baked-in has no known output cap: {row}"
    );
    assert!(
        row["fetched_at"].is_null(),
        "baked-in rows were never fetched: {row}"
    );
}

/// Builtins are untouched by any of this.
#[test]
fn models_refresh_builtin_name_still_resolves() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    std::fs::create_dir_all(home).unwrap();
    // No credentials (or only the closed port below), so the refresh
    // reports a skip — but it must resolve the name and attempt it. With
    // the only target failed, the command fails.
    models_cmd(home)
        // Same hermetic seam as above: never the real vendor endpoint.
        .env(
            "RUPU_ANTHROPIC_BASE_URL_OVERRIDE",
            "http://127.0.0.1:9/v1/messages",
        )
        .args(["models", "refresh", "--provider", "anthropic"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("skip anthropic"));
}

# Device Login Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `rupu auth login --mode device` logs in to OpenAI, Anthropic, GitLab, Gemini, GitHub and Copilot from a machine with no browser, storing the same credential `--mode sso` stores.

**Architecture:** `ProviderOAuth` gains a `headless` flow next to its `flow`; the CLI picks it on `--mode device`. Three flow modules in `rupu-auth`'s `oauth/` do the work — the generalised RFC 8628 `device.rs` (GitHub, Copilot, GitLab), a new `paste.rs` (Anthropic and Gemini: authorize URL with a vendor-hosted display page as the redirect, code pasted back) and a new `openai_device.rs` (Codex CLI's proprietary device auth) — over a shared `common.rs` holding the token exchange and the token-response → `StoredCredential` mapping lifted out of `callback.rs`. The credential is stored in the account's `sso` slot.

**Tech Stack:** Rust 2021, `reqwest` through `rupu_netflow::http::client_with` (NullSink, as every auth call), `serde`, `tokio`, `httpmock` + `serial_test` in tests, `assert_cmd` for CLI tests.

**Spec:** `docs/superpowers/specs/2026-11-08-rupu-device-login-design.md`

## Global Constraints

- `rupu_providers::AuthMode` stays `ApiKey | Sso`; a device login stores under `AuthMode::Sso`; `auth status` is unchanged (spec D1).
- No `--mode sso` flow changes behaviour or request shape, except the headless refusal's wording (spec non-goals, D5).
- Every HTTP client in these flows is `rupu_netflow::http::client_with(FlowCtx::system(Origin::System), reqwest::Client::builder(), Arc::new(NullSink))` (matt's ruling: login traffic is outside netflow).
- Never print or log a pasted code, a device code or a token.
- Workspace deps only; `#![deny(clippy::all)]`; no new top-level `tests/*.rs` (modules under `tests/it/`, listed in `main.rs`); every env-mutating test is `#[serial]`.
- Test seams (spec D4): `RUPU_DEVICE_DEVICE_URL_OVERRIDE`, `RUPU_DEVICE_TOKEN_URL_OVERRIDE` (RFC 8628 flows, built-in applications only), `RUPU_DEVICE_FAST_POLL` (RFC 8628 flows, a chosen GitLab application included: it is a sleep seam, not an endpoint), `RUPU_OAUTH_TOKEN_URL_OVERRIDE` (paste flows' exchange, built-in applications only), `RUPU_OPENAI_DEVICE_ISSUER_OVERRIDE` (OpenAI issuer). A chosen GitLab application's endpoints are never overridden: the two URL-override seams never apply to it.
- Messages, verbatim from the spec:
  - headless refusal (callback.rs): `SSO requires a desktop browser. Run with --mode device to log in from any browser, or --mode api-key.`
  - no headless flow: `<kind> has no device login; use --mode api-key` (an `openai-compatible` account is refused earlier, by the pre-existing `openai-compatible providers only support --mode api-key` — the same meaning, for `--mode sso` and `--mode device` alike)
  - GitLab 404: `this GitLab does not offer device login (it needs GitLab 17.9 or later); log in from a desktop with --mode sso, or use --mode api-key with a personal access token`
  - OpenAI 404: `device login is not enabled for this ChatGPT account. Turn it on in ChatGPT settings → Security (a workspace admin does it under workspace permissions), then retry; or log in from a desktop with --mode sso`
  - OpenAI deadline: `device login timed out`
  - pasted state mismatch: `pasted state does not match this login; start again`
  - Gemini no refresh token: `Google returned no refresh token. Revoke "Gemini CLI" at https://myaccount.google.com/permissions and log in again, or use --mode api-key`
- Priority order is the task order: OpenAI (Task 2), Anthropic (Task 3), GitLab (Task 4), Gemini (Task 5). Each task leaves `main`-mergeable code.
- Commits: `git -c commit.gpgsign=false commit` if signing fails; never `cargo fmt` a `lib.rs`/`main.rs`/`mod.rs` (repo rule); never bare `git stash`.

## Review Focus

1. A pasted value with surrounding whitespace, quotes or backticks (terminals and chat clients add them): parsed the same as the bare value. Test in Task 3.
2. An OpenAI `interval` that arrives as a JSON number instead of Codex's string: still parsed, not a deserialization failure that aborts the login. Test in Task 2.
3. A GitLab `slow_down` reply without an `interval` field: the interval grows by 5 s, the flow does not spin. Test in Task 4.
4. A Gemini token reply with `refresh_token` present but empty: refused like a missing one, nothing stored. Test in Task 5.
5. `rupu auth logout --mode device`: removes the `sso` entry (same slot), never errors on the unknown-mode path. Test in Task 1.

---

### Task 1: The `device` mode word, `HeadlessFlow`, and the shared `common.rs`

**Files:**
- Create: `crates/rupu-auth/src/oauth/common.rs`
- Modify: `crates/rupu-auth/src/oauth/providers.rs` (enum + field + table), `crates/rupu-auth/src/oauth/callback.rs` (use `common`, headless message), `crates/rupu-auth/src/oauth/mod.rs`, `crates/rupu-cli/src/cmd/auth.rs` (`AuthModeArg::Device`, login dispatch, logout text), `crates/rupu-auth/tests/it/oauth_callback.rs` (headless message assertion if present)
- Test: `crates/rupu-auth/src/oauth/providers.rs` (unit), `crates/rupu-cli/tests/it/auth_login_modes.rs`

**Interfaces:**
- Produces, in `providers.rs`:
  ```rust
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub enum HeadlessFlow {
      /// RFC 8628 device code (GitHub, Copilot, GitLab).
      DeviceCode,
      /// Authorization code shown on a vendor-hosted page the user copies from.
      PasteCode {
          redirect_uri: &'static str,
          /// Appended after `ProviderOAuth::extra_authorize_params`.
          extra_authorize_params: &'static [(&'static str, &'static str)],
      },
      /// Codex CLI's `--device-auth` (OpenAI accounts endpoints).
      OpenAiDeviceAuth,
  }
  ```
  `ProviderOAuth` gains `pub headless: Option<HeadlessFlow>`. In this task: `Some(DeviceCode)` for `Github` and `Copilot`, `None` for everything else (later tasks fill their vendor in). `OAuthClient` gains `pub device_url: Option<String>` (`None` everywhere in this task; `gitlab::oauth_client` fills it in Task 4).
- Produces, in `common.rs` (all `pub(crate)` unless noted):
  ```rust
  pub(crate) struct TokenResponse { /* moved verbatim from callback.rs */ }
  pub(crate) fn stored_credential_for(provider: ProviderId, token: TokenResponse) -> StoredCredential; // moved
  /// Records a chosen application's id and token endpoint (EXTRA_CLIENT_ID / EXTRA_TOKEN_URL) on an OAuth credential.
  pub(crate) fn record_client(stored: &mut StoredCredential, app: &OAuthClient);
  /// The one HTTP client every auth flow uses (NullSink).
  pub(crate) fn http_client() -> anyhow::Result<reqwest::Client>;
  /// The browser flow's authorize URL, with `extra` appended after `oauth.extra_authorize_params`.
  pub(crate) fn build_authorize_url(oauth: &ProviderOAuth, app: &OAuthClient, challenge: &str, state: &str, redirect_uri: &str, extra: &[(&str, &str)]) -> anyhow::Result<String>;
  pub(crate) struct CodeExchange<'a> { pub oauth: &'a ProviderOAuth, pub app: &'a OAuthClient, pub client_secret: Option<&'a str>, pub code: &'a str, pub redirect_uri: &'a str, pub code_verifier: &'a str, pub state: &'a str }
  /// POSTs the authorization-code grant in `oauth.token_body_format`, adding `client_secret` when given and `state` when `oauth.include_state_in_token_body`.
  pub(crate) async fn exchange_code(client: &reqwest::Client, x: CodeExchange<'_>) -> anyhow::Result<TokenResponse>;
  ```
  `callback.rs` keeps its public API (`run`, `run_with_client`) and calls these; its private `TokenResponse`, `stored_credential_for`, `build_authorize_url` and the inline exchange are deleted.
- Produces, in `cmd/auth.rs`: `AuthModeArg::Device` (clap name `device`, help "log in from a browser on another machine"); `From<AuthModeArg> for AuthMode` maps `Device => Sso`; `login`'s `match mode` gains a `Device` arm that resolves `oauth.headless` and dispatches:
  ```rust
  Some(HeadlessFlow::DeviceCode) => rupu_auth::oauth::device::run_with_client(pid, app).await?,   // Task 4 adds run_with_client; this task calls device::run(pid)
  Some(HeadlessFlow::PasteCode { .. }) => rupu_auth::oauth::paste::run(pid, app).await?,          // Task 3
  Some(HeadlessFlow::OpenAiDeviceAuth) => rupu_auth::oauth::openai_device::run().await?,          // Task 2
  None => anyhow::bail!("{kind} has no device login; use --mode api-key"),
  ```
  The enum has all three variants from this task on. In this task the `PasteCode` and `OpenAiDeviceAuth` arms `bail!("{kind} device login is not available in this build")` (no vendor's table entry names them yet, so the arms are unreachable in practice but never `unreachable!`); Tasks 2 and 3 each replace one line. The post-flow steps (store under `AuthMode::Sso`, print `rupu: stored {account} sso credential`, Gemini `set_up_code_assist`) are shared with the `Sso` arm through one helper: `async fn store_sso_login(resolver: &dyn CredentialResolver, account: &str, pid: ProviderId, stored: StoredCredential) -> anyhow::Result<()>`.

- [ ] **Step 1: Write the failing provider-table unit tests** in `providers.rs`'s `mod tests`

```rust
#[test]
fn github_and_copilot_headless_is_the_device_code_flow() {
    for p in [ProviderId::Github, ProviderId::Copilot] {
        assert_eq!(provider_oauth(p).unwrap().headless, Some(HeadlessFlow::DeviceCode), "{p}");
    }
}

#[test]
fn vendors_without_a_headless_flow_yet_say_so() {
    // Filled in by later tasks: OpenAI (Task 2), Anthropic (3), GitLab (4), Gemini (5).
    for p in [ProviderId::Anthropic, ProviderId::Openai, ProviderId::Gemini, ProviderId::Gitlab] {
        assert_eq!(provider_oauth(p).unwrap().headless, None, "{p}");
    }
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p rupu-auth --lib headless`
Expected: compile error, `headless` has no field.

- [ ] **Step 3: Add `HeadlessFlow`, the `headless` field, `OAuthClient::device_url`** in `providers.rs`; set `headless` per vendor as the Interfaces block says; set `device_url: None` wherever an `OAuthClient` is built (`callback.rs`, `gitlab.rs`, tests).

- [ ] **Step 4: Create `common.rs`** with the moved `TokenResponse`, `stored_credential_for`, `build_authorize_url` (now taking `extra`), and the new `record_client`, `http_client`, `exchange_code`; register it in `oauth/mod.rs`; rewrite `callback.rs` to call them (its own `TokenResponse`/`stored_credential_for`/`build_authorize_url` and the inline exchange go away; behaviour identical). Change the headless refusal text to the Global Constraints wording.

- [ ] **Step 5: Run the auth crate's tests to verify nothing moved behaviour**

Run: `cargo test -p rupu-auth`
Expected: all pass, including `oauth_callback` and `oauth_device`. If an existing test asserts the old headless message (`Run with --mode api-key for headless setups`), update it to the new wording.

- [ ] **Step 6: Write the failing CLI tests** in `crates/rupu-cli/tests/it/auth_login_modes.rs`

```rust
#[test]
fn login_device_for_a_vendor_without_a_headless_flow_refuses() {
    let tmp = assert_fs::TempDir::new().unwrap();
    Command::cargo_bin("rupu").unwrap()
        .env("RUPU_HOME", tmp.path())
        .args(["auth", "login", "--account", "anthropic", "--mode", "device"])
        .assert().failure()
        .stderr(predicate::str::contains("anthropic has no device login; use --mode api-key"));
    assert!(!tmp.path().join("auth.json").exists());
}
// NOTE for Task 3: this test flips to a success test when Anthropic gains its paste flow; Task 3 rewrites it.

#[test]
fn login_device_for_github_runs_the_device_code_flow_and_stores_sso() {
    // httpmock server: POST /login/device/code -> {device_code, user_code, verification_uri, expires_in: 900, interval: 0}
    //                  POST /login/oauth/access_token -> {access_token: "ghp-test", token_type: "bearer", scope: "repo"}
    // env RUPU_HOME=tmp, RUPU_DEVICE_DEVICE_URL_OVERRIDE, RUPU_DEVICE_TOKEN_URL_OVERRIDE, RUPU_DEVICE_FAST_POLL=1
    // args: auth login --account github --mode device
    // assert success, stdout contains "enter code: " and "stored github sso credential"
    // then: auth status --format json (or read tmp/auth.json) shows an sso entry for github and no api-key entry
}

#[test]
fn logout_device_removes_the_sso_entry() {
    // seed via: auth login --account anthropic --mode api-key --key sk-x   (api-key entry)
    // and a device login for github as above (sso entry)
    // auth logout --account github --mode device -> success
    // auth.json no longer has github's sso entry; anthropic's api-key entry remains
}

#[test]
fn login_sso_headless_refusal_names_device_mode() {
    // same gate as login_sso_without_browser_errors_on_headless_linux (Linux, no DISPLAY)
    // stderr contains "Run with --mode device to log in from any browser, or --mode api-key."
}
```

- [ ] **Step 7: Run them to verify they fail**

Run: `cargo test -p rupu-cli --test it auth_login_modes::`
Expected: FAIL — clap rejects `device` as a `--mode` value.

- [ ] **Step 8: Implement `AuthModeArg::Device`, the `From` mapping, the `Device` login arm, `store_sso_login`, and the logout text** in `cmd/auth.rs` (the openai-compatible logout check's message becomes `openai-compatible providers only support api-key credentials` so it is right for `device` too).

- [ ] **Step 9: Run the CLI tests to verify they pass**

Run: `cargo test -p rupu-cli --test it auth_login_modes::`
Expected: PASS (the headless one is skipped off Linux).

- [ ] **Step 10: Update the operator docs' mode surface**: `docs/providers.md` matrix gains a `Device` column (`✓` for copilot, `—` elsewhere for now, each later task flips its row) and a new `### Device login (--mode device)` section under "Auth flows" stating what the mode is, that the credential is stored as `sso`, and the per-vendor list (filled per task); README's "Authenticate" table gains a `Device` column likewise; `docs/scm.md`'s sentence "GitHub uses a device-code SSO flow; GitLab uses browser-callback PKCE" gets "(`--mode device` is the same flow for GitHub)". Add the spec to CLAUDE.md's "Read first" list.

- [ ] **Step 11: Commit**

```bash
git add crates/rupu-auth crates/rupu-cli docs/providers.md README.md docs/scm.md CLAUDE.md
git commit -m "auth: --mode device, HeadlessFlow and the shared oauth common module"
```

---

### Task 2: OpenAI device auth

**Files:**
- Create: `crates/rupu-auth/src/oauth/openai_device.rs`
- Modify: `crates/rupu-auth/src/oauth/providers.rs` (OpenAI `headless: Some(OpenAiDeviceAuth)`, test), `crates/rupu-auth/src/oauth/mod.rs`, `crates/rupu-cli/src/cmd/auth.rs` (one arm), `docs/providers.md`, `docs/providers/openai.md`, `README.md`
- Test: `crates/rupu-auth/tests/it/openai_device.rs` (+ `main.rs` entry), `crates/rupu-cli/tests/it/auth_login_modes.rs`

**Interfaces:**
- Consumes: `common::{http_client, exchange_code, CodeExchange, stored_credential_for}`, `provider_oauth(ProviderId::Openai)` (client id, `token_body_format: Form`).
- Produces:
  ```rust
  pub const DEFAULT_ISSUER: &str = "https://auth.openai.com";
  pub struct Options {
      pub issuer: String,            // trailing '/' trimmed
      pub client_id: String,         // provider_oauth(Openai).client_id
      pub budget: std::time::Duration, // 15 min
  }
  impl Options {
      /// DEFAULT_ISSUER unless RUPU_OPENAI_DEVICE_ISSUER_OVERRIDE is set.
      pub fn from_env() -> Self;
  }
  pub async fn run() -> anyhow::Result<StoredCredential>;             // run_with(Options::from_env())
  pub async fn run_with(opts: Options) -> anyhow::Result<StoredCredential>;
  /// The page the operator opens: `{issuer}/codex/device`.
  pub fn verification_url(issuer: &str) -> String;
  ```
  Wire (spec D3): `POST {issuer}/api/accounts/deviceauth/usercode` JSON `{"client_id"}` → `{device_auth_id, user_code (alias usercode), interval}` where `interval` deserializes from a string or a number (default 0); 404 → the OpenAI 404 message; other non-2xx → `device login request failed with status {status}`. Print `Visit {verification_url} and enter code: {user_code}` then `Only continue if you started this login yourself. The code expires in 15 minutes.` then `Waiting for approval…`. Poll `POST {issuer}/api/accounts/deviceauth/token` JSON `{device_auth_id, user_code}`: 403/404 → sleep `interval` s and retry; past `budget` since the first poll → the deadline message; other non-2xx → `device login failed with status {status}`; 2xx → `{authorization_code, code_verifier, code_challenge}`. Exchange through `common::exchange_code` with `app.token_url = {issuer}/oauth/token`, `redirect_uri = {issuer}/deviceauth/callback`, `code_verifier` = the server's, `state = ""` (OpenAI's `include_state_in_token_body` is false). Return `stored_credential_for(ProviderId::Openai, token)` (keeps `id_token`).

- [ ] **Step 1: Write the failing integration tests** in `crates/rupu-auth/tests/it/openai_device.rs` (add `mod openai_device;` to `main.rs`). Use httpmock; the issuer is `server.base_url()`; no env needed (call `run_with`).

```rust
#[tokio::test]
async fn device_auth_completes_and_stores_the_id_token() {
    // usercode mock -> {"device_auth_id":"da-1","user_code":"ABCD-1234","interval":"0"}
    // token mock: first two calls 403 (use httpmock's `hits` or two mocks with .then_status via sequence), then 200 {"authorization_code":"ac-1","code_verifier":"srv-verifier","code_challenge":"srv-challenge"}
    //   (simplest: a single mock that always returns 200; add a second test for pending below)
    // /oauth/token mock: expects form body containing grant_type=authorization_code, code=ac-1, code_verifier=srv-verifier, redirect_uri=<issuer>/deviceauth/callback, client_id=app_EMoamEEZ73f0CkXaXp7hrann
    //   -> {"access_token":"at","refresh_token":"rt","id_token":"idt","expires_in":3600}
    // assert: credentials OAuth access "at"; refresh_token Some("rt"); extra["id_token"] == "idt"; all mocks hit
}

#[tokio::test]
async fn pending_replies_are_polled_until_approval() {
    // token-poll mock returns 403 for the first 2 hits then 200 (httpmock: two mocks, one matching a header you can't vary — instead use `MockServer` with a shared AtomicUsize in a custom matcher closure via `when.matches(...)`)
    // assert the poll mock was hit >= 3 times and the flow succeeded
}

#[tokio::test]
async fn interval_as_a_number_is_accepted() {
    // usercode mock -> {"device_auth_id":"da-1","user_code":"X","interval":0}
    // rest as the success test; assert success
}

#[tokio::test]
async fn a_404_on_usercode_says_device_login_is_not_enabled() {
    // usercode mock -> 404
    // assert err text contains "device login is not enabled for this ChatGPT account" and "ChatGPT settings"
}

#[tokio::test]
async fn the_budget_ends_a_login_nobody_approves() {
    // usercode -> interval "0"; token poll always 403; Options { budget: Duration::from_millis(300), .. }
    // assert err text == contains "device login timed out"
}

#[tokio::test]
async fn an_unexpected_poll_status_fails_the_login() {
    // token poll -> 500; assert err contains "device login failed with status 500"
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p rupu-auth --test it openai_device::`
Expected: compile error, no module `openai_device`.

- [ ] **Step 3: Implement `openai_device.rs`** per the Interfaces block; set `headless: Some(HeadlessFlow::OpenAiDeviceAuth)` on OpenAI in `providers.rs` and move `Openai` out of `vendors_without_a_headless_flow_yet_say_so` into a new `openai_headless_is_codex_device_auth` test; replace the CLI's `OpenAiDeviceAuth` arm with `rupu_auth::oauth::openai_device::run().await?`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p rupu-auth --test it openai_device::` and `cargo test -p rupu-auth --lib headless`
Expected: PASS.

- [ ] **Step 5: Write the failing CLI test** in `auth_login_modes.rs`

```rust
#[test]
fn login_device_for_openai_stores_an_sso_credential() {
    // httpmock on the three OpenAI paths as the success test above (interval "0")
    // env RUPU_HOME=tmp, RUPU_OPENAI_DEVICE_ISSUER_OVERRIDE=server.base_url()
    // args: auth login --account openai --mode device
    // assert success; stdout contains "codex/device" and "enter code: ABCD-1234" and "stored openai sso credential"
    // auth.json has an openai sso entry whose extra.id_token == "idt"
}
```

- [ ] **Step 6: Run it, verify it fails, then verify it passes** (it should pass once Step 3 is in; if it fails, the env seam is not read — fix `Options::from_env`).

Run: `cargo test -p rupu-cli --test it auth_login_modes::login_device_for_openai`
Expected: PASS.

- [ ] **Step 7: Docs**: `docs/providers.md` matrix row `openai` Device `✓ (Codex device auth)`; the Device login section gets the OpenAI subsection (steps, the "enable in ChatGPT settings → Security" note, the phishing warning); `docs/providers/openai.md` gets a `## Device login` section after the ChatGPT SSO one; README row.

- [ ] **Step 8: Commit**

```bash
git add crates/rupu-auth crates/rupu-cli docs/providers.md docs/providers/openai.md README.md
git commit -m "auth: OpenAI device login (Codex CLI's device auth)"
```

---

### Task 3: Anthropic paste flow

**Files:**
- Create: `crates/rupu-auth/src/oauth/paste.rs`
- Modify: `crates/rupu-auth/src/oauth/providers.rs` (Anthropic `headless`), `crates/rupu-auth/src/oauth/mod.rs`, `crates/rupu-cli/src/cmd/auth.rs` (one arm), `docs/providers.md`, `docs/providers/anthropic.md`, `README.md`
- Test: `crates/rupu-auth/src/oauth/paste.rs` (unit: parsing), `crates/rupu-auth/tests/it/oauth_paste.rs` (+ `main.rs`), `crates/rupu-cli/tests/it/auth_login_modes.rs`

**Interfaces:**
- Consumes: `common::{http_client, build_authorize_url, exchange_code, CodeExchange, stored_credential_for, record_client}`, `PkcePair`, `HeadlessFlow::PasteCode { redirect_uri, extra_authorize_params }`.
- Produces:
  ```rust
  /// Where the pasted code comes from. Production: stdin (TTY or not).
  pub trait CodeInput {
      /// Shows `authorize_url` to the operator and returns what they pasted. `state` is what this login sent (tests build `code#state` from it).
      fn code(&mut self, authorize_url: &str, state: &str) -> anyhow::Result<String>;
  }
  /// Prints `Open this URL in any browser and sign in:\n\n  {url}\n\nPaste the code the page shows: ` and reads one line from stdin.
  pub struct StdinInput;
  #[derive(Debug, PartialEq, Eq)]
  pub struct Pasted { pub code: String, pub state: Option<String> }
  /// Trims whitespace and surrounding quotes/backticks; a URL → its `code`/`state` query; a `code=…` query string → the same; `code#state`; else a bare code. Empty → error `nothing was pasted`.
  pub fn parse_pasted(input: &str) -> anyhow::Result<Pasted>;
  pub async fn run(provider: ProviderId, client: Option<OAuthClient>) -> anyhow::Result<StoredCredential>;   // run_with_input(.., &mut StdinInput)
  pub async fn run_with_input(provider: ProviderId, client: Option<OAuthClient>, input: &mut dyn CodeInput) -> anyhow::Result<StoredCredential>;
  ```
  `run_with_input`: `provider_oauth` must have `headless: Some(PasteCode{..})` else `bail!("provider {provider} has no paste-code flow")`; app = chosen client or the built-in with `RUPU_OAUTH_TOKEN_URL_OVERRIDE` honoured for the built-in only (as `callback.rs`); PKCE + state (`state_is_verifier` honoured); URL via `build_authorize_url(.., redirect_uri, extra_authorize_params)`; `input.code(url, &state)`; `parse_pasted`; a pasted `state` that differs → the mismatch message, no exchange; `exchange_code` with the paste `redirect_uri` and rupu's state; `stored_credential_for`, then `record_client` when a client was chosen. Gemini's refresh-token check is Task 5's.

- [ ] **Step 1: Write the failing parsing unit tests** in `paste.rs`

```rust
#[test] fn code_hash_state_splits_at_the_first_hash() { assert_eq!(parse_pasted("abc#st").unwrap(), Pasted{code:"abc".into(), state:Some("st".into())}); assert_eq!(parse_pasted("a#b#c").unwrap().state.as_deref(), Some("b#c")); }
#[test] fn a_bare_code_has_no_state() { assert_eq!(parse_pasted("abc").unwrap(), Pasted{code:"abc".into(), state:None}); }
#[test] fn a_redirect_url_is_read_for_code_and_state() { let p = parse_pasted("https://platform.claude.com/oauth/code/callback?code=abc&state=st").unwrap(); assert_eq!(p, Pasted{code:"abc".into(), state:Some("st".into())}); }
#[test] fn a_query_string_is_read_for_code_and_state() { assert_eq!(parse_pasted("code=abc&state=st").unwrap(), Pasted{code:"abc".into(), state:Some("st".into())}); }
#[test] fn whitespace_quotes_and_backticks_are_stripped() { for s in ["  abc#st \n", "\"abc#st\"", "`abc#st`", "'abc#st'"] { assert_eq!(parse_pasted(s).unwrap().code, "abc", "{s:?}"); } }
#[test] fn an_empty_paste_is_an_error() { assert!(parse_pasted("  \n").unwrap_err().to_string().contains("nothing was pasted")); }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p rupu-auth --lib paste::`
Expected: compile error, no module.

- [ ] **Step 3: Implement `parse_pasted`, `CodeInput`, `StdinInput`, `run`, `run_with_input`** in `paste.rs`; register in `mod.rs`.

- [ ] **Step 4: Run the unit tests to verify they pass**

Run: `cargo test -p rupu-auth --lib paste::`
Expected: PASS.

- [ ] **Step 5: Write the failing flow tests** in `tests/it/oauth_paste.rs` (`#[serial]`, they set `RUPU_OAUTH_TOKEN_URL_OVERRIDE`; a `CodeInput` impl over a closure `FnMut(&str, &str) -> anyhow::Result<String>`)

```rust
#[tokio::test] #[serial]
async fn anthropic_paste_login_exchanges_with_the_hosted_redirect() {
    // token mock: POST /token, JSON body with grant_type authorization_code, code "abc", redirect_uri "https://platform.claude.com/oauth/code/callback", state == the state passed to the input, code_verifier == that same state (Anthropic: state_is_verifier)
    //   -> {"access_token":"sk-ant-oat01-x","refresh_token":"rt","expires_in":28800,"account":{"uuid":"acc"},"organization":{"uuid":"org"}}
    // input: |url, state| { assert!(url.starts_with("https://claude.ai/oauth/authorize?")); assert!(url.contains("code=true")); assert!(url.contains("redirect_uri=https%3A%2F%2Fplatform.claude.com%2Foauth%2Fcode%2Fcallback")); Ok(format!("abc#{state}")) }
    // assert stored access, refresh Some("rt"), extra account_uuid "acc", organization_uuid "org"; mock hit once
}

#[tokio::test] #[serial]
async fn a_mismatched_pasted_state_is_refused_before_any_exchange() {
    // input returns "abc#not-the-state"; assert err contains "pasted state does not match this login"; token mock hits == 0
}

#[tokio::test] #[serial]
async fn a_bare_code_proceeds_with_rupu_s_own_state() {
    // input returns "abc"; token mock expects state == the login's state; assert success
}

#[tokio::test] #[serial]
async fn a_failed_exchange_surfaces_the_vendor_message_and_stores_nothing() {
    // token mock -> 400 {"type":"error","error":{"type":"invalid_grant","message":"Invalid code"}}
    // assert err contains "Invalid code" (surface the body's error.message, else the status)
}
```

- [ ] **Step 6: Run to verify they fail, then set Anthropic's `headless`** to `Some(HeadlessFlow::PasteCode { redirect_uri: "https://platform.claude.com/oauth/code/callback", extra_authorize_params: &[] })` in `providers.rs` (move `Anthropic` out of the `None` test into `anthropic_headless_is_the_paste_flow`); replace the CLI's `PasteCode` arm with `rupu_auth::oauth::paste::run(pid, app).await?` (where `app` is `sso_client(..)?`, `None` for Anthropic); make the error surfacing read Anthropic's nested `error.message`.

Run: `cargo test -p rupu-auth --test it oauth_paste::`
Expected: PASS.

- [ ] **Step 7: Rewrite the CLI test** `login_device_for_a_vendor_without_a_headless_flow_refuses` to use `--account gemini` (still `None` until Task 5; Task 5 moves it to an `openai-compatible` account) and add:

```rust
#[test]
fn login_device_for_anthropic_reads_the_code_from_a_pipe() {
    // token mock as the flow test; env RUPU_HOME=tmp, RUPU_OAUTH_TOKEN_URL_OVERRIDE=server.url("/token")
    // .write_stdin("abc\n")  (a bare code: a CLI test cannot see the state)
    // args: auth login --account anthropic --mode device
    // assert success; stdout contains "claude.ai/oauth/authorize" and "Paste the code"; auth.json has anthropic sso entry
}
```

- [ ] **Step 8: Run the CLI tests to verify they pass**

Run: `cargo test -p rupu-cli --test it auth_login_modes::`
Expected: PASS.

- [ ] **Step 9: Docs**: `docs/providers.md` matrix `anthropic` Device `✓ (paste the code)`; Device login section's Anthropic subsection (what the page shows, paste `code#state`, expires in minutes, rerun on failure); `docs/providers/anthropic.md` `## Device login`; README row.

- [ ] **Step 10: Commit**

```bash
git add crates/rupu-auth crates/rupu-cli docs/providers.md docs/providers/anthropic.md README.md
git commit -m "auth: Anthropic device login (Claude Code's paste-the-code flow)"
```

---

### Task 4: GitLab device grant

**Files:**
- Modify: `crates/rupu-auth/src/oauth/device.rs` (generalise), `crates/rupu-auth/src/oauth/gitlab.rs` (`device_url`), `crates/rupu-auth/src/oauth/providers.rs` (GitLab `headless`), `crates/rupu-cli/src/cmd/auth.rs` (`DeviceCode` arm passes the chosen app; GitLab's `cfg_error` guard applies to `Device` as to `Sso`), `docs/scm.md`, `docs/providers.md`, `README.md`
- Test: `crates/rupu-auth/src/oauth/device.rs` (unit), `crates/rupu-auth/src/oauth/gitlab.rs` (unit), `crates/rupu-auth/tests/it/oauth_device.rs`, `crates/rupu-cli/tests/it/auth_login_gitlab.rs`

**Interfaces:**
- Consumes: `common::{http_client, stored_credential_for, record_client}`, `OAuthClient::device_url`.
- Produces:
  ```rust
  pub async fn run(provider: ProviderId) -> anyhow::Result<StoredCredential>;   // unchanged: run_with_client(provider, None)
  /// `client`: a chosen application (GitLab's configured one or a self-managed instance's), whose `device_url`, `token_url` and `client_id` are used as they are (no env override). `None`: the built-in application with the RUPU_DEVICE_* seams.
  pub async fn run_with_client(provider: ProviderId, client: Option<OAuthClient>) -> anyhow::Result<StoredCredential>;
  /// RFC 8628 §3.5: `slow_down` with no new interval adds 5 s; a reply's `interval` replaces the current one.
  pub fn next_interval(current: u64, reply: Option<u64>) -> u64;
  ```
  `run_with_client` accepts `provider_oauth(provider).flow == Device || headless == Some(DeviceCode)`. Form bodies as today. Pending: `authorization_pending` (interval unchanged), `slow_down` (`next_interval`); terminal: `expired_token`, `access_denied`, anything else → `device-code flow error: {error}` as today. A non-2xx on the device-code request: 404 and provider GitLab → the GitLab 404 message; else as today. Storage: GitHub/Copilot byte-identical to today's `StoredCredential` (the existing `device_code_flow_completes` test pins it); GitLab → `stored_credential_for(ProviderId::Gitlab, TokenResponse{..})` + `record_client` when a client was chosen. Prints `Visit {verification_uri} and enter code: {user_code}` then `Waiting for approval…`.
  `gitlab::oauth_client` sets `device_url: Some(format!("{instance}/oauth/authorize_device"))`.

- [ ] **Step 1: Write the failing unit tests**

In `device.rs`:
```rust
#[test] fn slow_down_without_an_interval_adds_five_seconds() { assert_eq!(next_interval(5, None), 10); }
#[test] fn a_reply_interval_replaces_the_current_one() { assert_eq!(next_interval(5, Some(7)), 7); }
```
In `gitlab.rs` tests: extend `gitlab_com_defaults_to_glab_s_application` and `a_self_managed_instance_s_endpoints_come_from_its_api_root` with `assert_eq!(c.device_url.as_deref(), Some("https://gitlab.com/oauth/authorize_device"))` / `Some("https://gitlab.example.com/oauth/authorize_device")`.
In `providers.rs` tests: `gitlab_headless_is_the_device_code_flow` (move GitLab out of the `None` test).

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p rupu-auth --lib -- device:: gitlab:: headless`
Expected: FAIL / compile error.

- [ ] **Step 3: Implement** `next_interval`, `run_with_client`, the GitLab storage and 404 branch in `device.rs`; `device_url` in `gitlab.rs`; GitLab's `headless` in `providers.rs`; the CLI `DeviceCode` arm becomes `device::run_with_client(pid, sso_client(&cfg, account, pid)?)` with the GitLab `cfg_error` guard copied from the `Sso` arm (extract it into `fn gitlab_config_guard(pid, cfg_error, cfg_path, account) -> anyhow::Result<()>` used by both arms).

- [ ] **Step 4: Run the unit tests to verify they pass**

Run: `cargo test -p rupu-auth --lib -- device:: gitlab:: headless`
Expected: PASS.

- [ ] **Step 5: Write the failing integration tests** in `tests/it/oauth_device.rs` (`#[serial]`; the chosen-client tests need no env seams)

```rust
#[tokio::test] #[serial]
async fn a_gitlab_device_login_uses_the_chosen_application_and_records_it() {
    // client = OAuthClient { client_id: "corp-app", authorize_url: ".../oauth/authorize", token_url: server.url("/oauth/token"), device_url: Some(server.url("/oauth/authorize_device")) }
    // authorize_device mock expects form client_id=corp-app and scope containing "api" -> {device_code:"dc", user_code:"0A44L90H", verification_uri:"https://gitlab.example.com/oauth/device", expires_in:300, interval:0}
    // token mock expects form grant_type=urn:ietf:params:oauth:grant-type:device_code, device_code=dc, client_id=corp-app, no client_secret -> 200 {access_token:"at", token_type:"Bearer", expires_in:7200, refresh_token:"rt", created_at: 1}
    // RUPU_DEVICE_FAST_POLL=1
    // assert: OAuth access "at"; refresh_token Some("rt"); expires_at within ~2h; extra[EXTRA_CLIENT_ID]=="corp-app"; extra[EXTRA_TOKEN_URL]==server.url("/oauth/token")
}

#[tokio::test] #[serial]
async fn gitlab_pending_and_slow_down_keep_polling_until_the_token() {
    // token mock sequence: 400 {"error":"authorization_pending"}, 400 {"error":"slow_down"}, 200 success (counter-based matcher)
    // assert success and hits == 3
}

#[tokio::test] #[serial]
async fn an_expired_device_code_ends_the_login() {
    // token mock -> 400 {"error":"expired_token"}; assert err contains "expired_token"
}

#[tokio::test] #[serial]
async fn a_gitlab_without_the_device_endpoint_names_the_version() {
    // authorize_device mock -> 404; assert err contains "GitLab 17.9 or later" and "--mode api-key"
}
```

- [ ] **Step 6: Run to verify they pass** (they exercise Step 3's code)

Run: `cargo test -p rupu-auth --test it oauth_device::`
Expected: PASS, including the pre-existing `device_code_flow_completes` unchanged.

- [ ] **Step 7: Add the CLI test** in `auth_login_gitlab.rs`

```rust
#[test]
fn device_login_for_a_self_managed_account_uses_its_application() {
    // config.toml: [scm.gl-corp] kind="gitlab" base_url="<server>/api/v4" oauth_client_id="corp-app"   (server = httpmock base_url, so the device/token endpoints are <server>/oauth/authorize_device and <server>/oauth/token)
    // mocks as the chosen-application flow test; env RUPU_HOME=tmp, RUPU_DEVICE_FAST_POLL=1
    // args: auth login --account gl-corp --mode device
    // assert success; stdout contains "enter code: 0A44L90H" and "stored gl-corp sso credential"; auth.json has oauth_client_id "corp-app"
}

#[test]
fn device_login_for_a_self_managed_account_without_a_client_id_says_what_to_register() {
    // same config as the sso test without oauth_client_id; args --mode device; same stderr assertions as the sso test; nothing stored
}
```

- [ ] **Step 8: Run them**

Run: `cargo test -p rupu-cli --test it auth_login_gitlab::`
Expected: PASS.

- [ ] **Step 9: Docs**: `docs/scm.md`: after the "GitHub uses a device-code SSO flow…" sentence, state that GitLab's `--mode device` is the device authorization grant (GitLab 17.9+, `glab auth login --device`'s flow, same gitlab.com application; a self-managed instance's application serves both modes and needs no redirect URI for device-only use); `docs/providers.md` Device login section's GitLab subsection; README row if GitLab is listed.

- [ ] **Step 10: Commit**

```bash
git add crates/rupu-auth crates/rupu-cli docs/scm.md docs/providers.md README.md
git commit -m "auth: GitLab device login (RFC 8628 device authorization grant)"
```

---

### Task 5: Gemini paste flow

**Files:**
- Modify: `crates/rupu-auth/src/oauth/providers.rs` (Gemini `headless`), `crates/rupu-auth/src/oauth/paste.rs` (refresh-token refusal), `crates/rupu-cli/src/cmd/auth.rs` (nothing new if `store_sso_login` already runs `set_up_code_assist` for Gemini — verify), `docs/providers.md`, `docs/providers/gemini.md`, `README.md`
- Test: `crates/rupu-auth/tests/it/oauth_paste.rs`, `crates/rupu-cli/tests/it/auth_login_modes.rs`, `crates/rupu-cli/tests/it/auth_login_gemini.rs` (if it has a seam for the Code Assist setup, reuse it)

**Interfaces:**
- Consumes: Task 3's `paste::run_with_input`, `common::*`.
- Produces: Gemini `headless: Some(HeadlessFlow::PasteCode { redirect_uri: "https://codeassist.google.com/authcode", extra_authorize_params: &[("access_type", "offline"), ("prompt", "consent")] })`. In `paste.rs`, after the exchange and before returning: when `provider == ProviderId::Gemini` and the token's `refresh_token` is `None` or empty → the Gemini no-refresh message, nothing returned. (Anthropic's browser flow tolerates a missing refresh token today; the paste flow keeps that for Anthropic.)

- [ ] **Step 1: Write the failing tests**

In `providers.rs`: `gemini_headless_is_the_paste_flow_with_consent` asserting the `redirect_uri` and that `extra_authorize_params` contains both pairs; remove Gemini from the `None` test (which now lists no vendor: delete it).

In `tests/it/oauth_paste.rs`:
```rust
#[tokio::test] #[serial]
async fn gemini_paste_login_sends_consent_params_and_the_client_secret() {
    // input: |url, _| { assert!(url.contains("access_type=offline")); assert!(url.contains("prompt=consent")); assert!(url.contains("redirect_uri=https%3A%2F%2Fcodeassist.google.com%2Fauthcode")); assert!(url.contains("code_challenge_method=S256")); Ok("4/0Acode".into()) }
    // token mock expects FORM body with code=4/0Acode, client_secret=<GEMINI_CLI_CLIENT_SECRET>, redirect_uri=https://codeassist.google.com/authcode, code_verifier present -> {"access_token":"ya29","refresh_token":"1//rt","expires_in":3599,"id_token":"x"}
    // assert stored refresh Some("1//rt"); extra["variant"] is the Gemini CLI hint; extra has no "id_token"
}

#[tokio::test] #[serial]
async fn a_gemini_reply_without_a_refresh_token_is_refused() {
    // token mock -> {"access_token":"ya29","expires_in":3599}; assert err contains "Google returned no refresh token" and "myaccount.google.com/permissions"
}

#[tokio::test] #[serial]
async fn a_gemini_reply_with_an_empty_refresh_token_is_refused() {
    // token mock -> {"access_token":"ya29","refresh_token":"","expires_in":3599}; same assertion
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p rupu-auth --test it oauth_paste::gemini` and `cargo test -p rupu-auth --lib headless`
Expected: FAIL (`headless` is `None` for Gemini; refusal missing).

- [ ] **Step 3: Implement** Gemini's `headless` entry and the refusal in `paste.rs`; confirm the CLI's `store_sso_login` runs `set_up_code_assist` for Gemini after a device login (it should, from Task 1; add it if not).

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test -p rupu-auth`
Expected: PASS.

- [ ] **Step 5: CLI test**: move `login_device_for_a_vendor_without_a_headless_flow_refuses` to an `openai-compatible` account (config `[providers.oai-local] kind = "openai-compatible" …` as other tests declare one; expected stderr `openai-compatible providers only support --mode api-key`), and add `login_device_for_gemini_reads_the_code_from_a_pipe` mirroring the Anthropic one (form-body token mock, `write_stdin("4/0Acode\n")`, stdout contains `accounts.google.com` and `Paste the code`). If the Code Assist setup would reach the network after the store, point it at a mock the way `auth_login_gemini.rs` does, or assert only that the credential was stored and the setup's failure is reported on stderr without failing the command (it never fails the login, by design).

Run: `cargo test -p rupu-cli --test it auth_login_modes::`
Expected: PASS.

- [ ] **Step 6: Docs**: `docs/providers.md` matrix `gemini` Device `✓ (paste the code)`; Device login section's Gemini subsection, including that the consent screen appears on every device login and the note "not yet verified against Google" to be removed after matt's live run; `docs/providers/gemini.md`: a `## Device login` section and change line 56 ("For headless setups where you can't run the SSO flow, use the AI Studio API-key path") to name `--mode device` first; README row.

- [ ] **Step 7: Commit**

```bash
git add crates/rupu-auth crates/rupu-cli docs/providers.md docs/providers/gemini.md README.md
git commit -m "auth: Gemini device login (Gemini CLI's paste-the-code flow)"
```

---

### Task 6: Whole-feature verification and the PR

**Files:** none new.

- [ ] **Step 1: Run the touched test binaries only** (repo rule: targeted tests, never a cold full-workspace run)

Run:
```bash
cargo test -p rupu-auth
cargo test -p rupu-cli --test it auth_login_modes:: auth_login_gitlab:: auth_login_gemini:: auth_status_table::
cargo clippy -p rupu-auth -p rupu-cli --all-targets
```
Expected: all green, no clippy warnings.

- [ ] **Step 2: Check the docs render coherently**: `docs/providers.md` has one Device login section with five vendor subsections (OpenAI, Anthropic, GitLab, Gemini, GitHub/Copilot) and the matrix has a Device column with `✓` on every row but `openai-compatible`.

- [ ] **Step 3: Push the branch and open the PR** (explicit refspec; repo `push.default=matching`)

```bash
git push origin HEAD:refs/heads/claude/rupu-device-login-support-f70faa
gh pr create --title "auth: --mode device, headless SSO for OpenAI, Anthropic, GitLab, Gemini" --body "<summary: spec link, per-vendor flow one line each, test plan, what needs matt's live run on the Linux VM>"
```

- [ ] **Step 4: Hand over the live-run checklist** to matt (OpenAI, Anthropic, GitLab, Gemini on the Linux VM with the beta binary; what each should print; what to send back if one fails: the exact stderr, no codes or tokens).

# Device login: headless SSO for every provider that can support it

**Status:** design, 2026-11-08. Operator doc to write: `docs/providers.md` (matrix + a new "Device login" section), `docs/scm.md`, `README.md` login table.

## Intent

matt runs rupu on machines with no browser: a Linux VM, the Kali build hosts, Sutajio over SSH. Today an SSO login there is impossible for every provider but Copilot and GitHub, because `rupu auth login --mode sso` for Anthropic, OpenAI, Gemini and GitLab is a PKCE flow that needs a browser on the same machine to land a `localhost` redirect. The only way out is an API key, which bills differently (Anthropic, OpenAI) or is not what the operator has (a ChatGPT or Claude subscription).

This design adds `rupu auth login --mode device`: a login that needs no browser on the machine running rupu. rupu prints a URL and, where the vendor issues one, a short code; the operator completes the login in a browser anywhere, and rupu ends up with the same credential the browser flow would have stored.

Each vendor offers exactly one headless path and they are all different. The design mirrors each vendor's own CLI (or the reference implementations rupu already copies) rather than inventing a flow: Codex CLI's `codex login --device-auth`, Claude Code's "paste the code" login, Gemini CLI's `NO_BROWSER=true` login, and `glab auth login --device`. Everything here was read from those sources on 2026-11-08 (research notes in the appendix).

## Priority

OpenAI, then Anthropic, then GitLab, then Gemini. The plan orders its tasks this way so the top of the list can ship on its own if the bottom slips.

## Goals

- `rupu auth login --account <a> [--kind <k>] --mode device` works for every vendor that has a headless login: OpenAI, Anthropic, GitLab (gitlab.com and self-managed), Gemini, GitHub and Copilot.
- The credential it stores is indistinguishable from the one `--mode sso` stores: same storage slot, same refresh path, same provider client behaviour, same `auth status` row. Nothing downstream of login learns a new word.
- A vendor with no headless path, or a login the vendor refuses (device login not enabled on a ChatGPT account, a GitLab older than 17.9), fails with one actionable message naming what to do instead.
- Every flow is testable without a browser or a vendor: a mock server per flow, and the pasted code read from stdin so a test (or an operator piping over SSH) needs no TTY.

## Non-goals

- Changing any `--mode sso` flow. The browser flows stay byte-for-byte what they are, Gemini's missing `prompt=consent` included.
- Auto-switching `--mode sso` to the device flow on a headless machine. The operator asks for `--mode device`; the headless refusal in the browser flow just names it.
- Anthropic Console / platform-account login (API-key minting through `org:create_api_key`). rupu's Anthropic SSO is the claude.ai subscription login and stays so.
- Claude Code's `setup-token` long-lived token, Google's limited-input device grant (impossible for Gemini: wrong client type and `cloud-platform` is not an allowed scope), an RFC 8628 grant for Anthropic (none exists).
- A control-plane or web login. `rupu auth login` is the only login surface, as today.
- `rupu auth status` changes. A device-obtained credential shows as `sso`.

## Anti-goals

- A flow that stores a credential it knows is lame: a Gemini login that came back without a refresh token (the operator consented to the Gemini CLI client before and Google omitted it) must fail with the fix, not store a one-hour token.
- A silent fallback. A vendor that answers "not enabled" or "not supported" is reported as such, never retried as a browser flow.
- Logging or echoing a pasted code or a token.

## The operator's view

```sh
rupu auth login --account openai --mode device
# Visit https://auth.openai.com/codex/device and enter code: ABCD-1234
# (Only continue if you started this login yourself. The code expires in 15 minutes.)
# Waiting for approval…
# rupu: stored openai sso credential

rupu auth login --account anthropic --mode device
# Open this URL in any browser and sign in:
#   https://claude.ai/oauth/authorize?code=true&client_id=…
# Paste the code the page shows:
# rupu: stored anthropic sso credential

rupu auth login --account gitlab --mode device
# Visit https://gitlab.com/oauth/device and enter code: 0A44L90H
# Waiting for approval…
# rupu: stored gitlab sso credential

rupu auth login --account gemini --mode device
# Open this URL in any browser and sign in:
#   https://accounts.google.com/o/oauth2/v2/auth?…
# Paste the code the page shows:
# rupu: stored gemini sso credential
# (then the Code Assist project setup, exactly as after --mode sso)
```

`--mode sso` on a headless Linux machine keeps refusing, with the message changed to name both ways out:

```
SSO requires a desktop browser. Run with --mode device to log in from any
browser, or --mode api-key.
```

`--mode device` on a vendor with no headless login (`local`, an `openai-compatible` provider, `linear`, `jira`) refuses with `<kind> has no device login; use --mode api-key`. An `openai-compatible` account is refused earlier, by the pre-existing `openai-compatible providers only support --mode api-key` (the same meaning, for `--mode sso` and `--mode device` alike), so it never reaches this refusal.

`rupu auth logout --mode device` means the same slot as `--mode sso` (see below) and is accepted as a spelling of it.

## Decisions

### D1. `device` is a login method, not a storage slot

`rupu_providers::AuthMode` stays `ApiKey | Sso`. The CLI's `AuthModeArg` gains `Device`, and it converts to `AuthMode::Sso`. A device login stores its credential under the account's `sso` entry, the resolver refreshes it through the same per-vendor path, an agent's `auth: sso` picks it, `auth status` shows `sso`. Rationale: the token is the same token; a third slot would make every consumer of `AuthMode` (resolver, agent frontmatter, status table, CP DTOs) learn a word that changes nothing for them.

### D2. One headless flow per vendor, chosen by the provider table

`ProviderOAuth` keeps `flow` (what `--mode sso` runs) and gains `headless: Option<HeadlessFlow>` (what `--mode device` runs):

| Vendor | `--mode sso` | `--mode device` (`headless`) |
|---|---|---|
| openai | Callback | `OpenAiDeviceAuth` |
| anthropic | Callback | `PasteCode { redirect_uri: "https://platform.claude.com/oauth/code/callback" }` |
| gitlab | Callback | `DeviceCode` (RFC 8628, `/oauth/authorize_device`) |
| gemini | Callback | `PasteCode { redirect_uri: "https://codeassist.google.com/authcode" }` |
| github, copilot | Device (RFC 8628) | `DeviceCode` (the same flow; `--mode sso` and `--mode device` are identical here) |
| local, linear, jira, openai-compatible | — | `None` → refusal |

The CLI's `login` matches on the mode word only: `Sso` → `oauth.flow` as today; `Device` → `oauth.headless` or the refusal. No vendor name in the CLI.

### D3. The three flows and where they live

All in `crates/rupu-auth/src/oauth/`, one module per flow, each returning a `StoredCredential` like `callback::run_with_client` does. The token-response → `StoredCredential` mapping (`stored_credential_for` in `callback.rs`, with the Anthropic account/org uuids, OpenAI's `id_token`, Gemini's variant hint, and the chosen-application `oauth_client_id` / `oauth_token_url` extras) moves to a shared `token.rs` so every flow stores exactly what the browser flow stores.

**`device.rs` — RFC 8628 device code (GitHub, Copilot, GitLab).** The existing module, generalised:

- Takes the `OAuthClient` the login chose (a GitLab account's configured application or self-managed instance, from `gitlab::oauth_client`), falling back to the built-in. `OAuthClient` gains `device_url: Option<String>`; `gitlab::oauth_client` sets it to `{instance}/oauth/authorize_device`.
- Request: `POST device_url` form `client_id`, `scope`. Shows `verification_uri` and `user_code` (the plain URI, the operator types the code: the RFC's anti-phishing posture and what glab shows).
- Poll: `POST token_url` form `grant_type=urn:ietf:params:oauth:grant-type:device_code`, `device_code`, `client_id`. `authorization_pending` keeps polling; `slow_down` keeps polling after adding 5 s to the interval when the reply carries no new `interval` (GitLab never sends one; GitHub does); `expired_token`, `access_denied` and anything else end the flow with the vendor's error. The deadline is `expires_in` (GitLab 300 s, GitHub 900 s).
- A 404 from `device_url` on GitLab: `this GitLab does not offer device login (it needs GitLab 17.9 or later); log in from a desktop with --mode sso, or use --mode api-key with a personal access token`.
- Storage: GitHub and Copilot unchanged (byte-compatible with today). GitLab through the shared mapping: `refresh_token`, `expires_at`, and the chosen application's `oauth_client_id` / `oauth_token_url` extras, so the existing GitLab refresh (`TokenSource`, rotation-aware) needs nothing new.
- A self-managed instance needs what it needs today: a non-confidential application with glab's scopes and its id in `[scm.<account>].oauth_client_id`. The redirect URI is not used by the device grant, but the registration message stays as it is (one application serves both modes).

**`paste.rs` — authorization code shown on a vendor-hosted page (Anthropic, Gemini).** The browser flow's authorize URL with the vendor's hosted display page as `redirect_uri`, no listener:

- Builds the authorize URL exactly as `callback.rs` does (`build_authorize_url`: PKCE S256, `state`, scopes, `extra_authorize_params` — Anthropic's `code=true` is already there) with the `redirect_uri` from `HeadlessFlow::PasteCode`. Prints it.
- Reads one line from stdin, TTY or not (so an operator can pipe it and a test can drive it). Accepts, trimmed: Anthropic's `code#state`; a full redirect URL or a bare `code=…&state=…` query string (what the page's address bar holds); or a bare code. A pasted `state` must equal the one sent, else the login fails before any exchange (`pasted state does not match this login; start again`). A pasted value without a state proceeds with the state rupu generated (Gemini's page shows only the code).
- Exchanges at `token_url` exactly as the browser flow does (`token_body_format`, `client_secret`, `include_state_in_token_body`, `code_verifier`), with the paste `redirect_uri`. One attempt; a failure prints the vendor's error message and ends the login (codes are single-use and short-lived; the operator reruns).
- Anthropic: `state` stays `state_is_verifier` as the browser flow has it (the proven request shape; the server accepts either, and the code is bound by PKCE). The token exchange goes through the same HTTP client as today (its User-Agent must not start with `claude-code/` or `Mozilla`, which the vendor rate-limits; today's client satisfies that).
- Gemini: PKCE S256 is what binds the code (the page shows no state), and is what the browser flow already sends. The paste flow's authorize URL adds two parameters the browser flow does not send: `access_type=offline` (what Gemini CLI's own manual login sends) and `prompt=consent`. Without `prompt=consent`, Google returns a refresh token only on an account's first consent to the Gemini CLI client, so every operator who has ever run `--mode sso` for Gemini (or the Gemini CLI itself) would get a one-hour token with no refresh on their first device login. `prompt=consent` makes Google show the consent screen and issue a refresh token on every login; a consent screen on each login is the normal cost of a CLI login and is what `gcloud` does. `HeadlessFlow::PasteCode` carries these as its own extra authorize parameters, appended after `extra_authorize_params`, so the browser flow's URL is untouched. After storing, `set_up_code_assist` runs as after `--mode sso`. A token reply that still carries no `refresh_token` fails the login: `Google returned no refresh token. Revoke "Gemini CLI" at https://myaccount.google.com/permissions and log in again, or use --mode api-key`. Nothing is stored.

**`openai_device.rs` — OpenAI's device auth (Codex CLI's `--device-auth`).** Not RFC 8628: a proprietary pair of accounts endpoints that ends in a normal authorization-code exchange with a server-generated PKCE verifier.

- Issuer `https://auth.openai.com`; `api_base = {issuer}/api/accounts`.
- `POST {api_base}/deviceauth/usercode`, JSON `{"client_id": <Codex client id>}`, `Content-Type: application/json` and nothing else (no `originator`, no Codex User-Agent: Codex uses a raw client here). Reply `{device_auth_id, user_code, interval}`; `interval` is a string (`"5"`), parsed, default 0. A 404 means device login is not enabled for the account: `device login is not enabled for this ChatGPT account. Turn it on in ChatGPT settings → Security (a workspace admin does it under workspace permissions), then retry; or log in from a desktop with --mode sso`.
- Shows `{issuer}/codex/device` (computed, not returned) and `user_code`, with Codex's warning: continue only if you started this login yourself; the code expires in 15 minutes.
- Polls `POST {api_base}/deviceauth/token`, JSON `{device_auth_id, user_code}`. 403 and 404 mean pending: sleep `interval` and retry. 15 minutes from the first poll ends the flow (`device login timed out`). Any other non-2xx ends it with the status. Success: `{authorization_code, code_verifier, code_challenge}`.
- Exchanges at `{issuer}/oauth/token`, form-encoded (the browser flow's `token_body_format`): `grant_type=authorization_code`, `client_id`, `code=<authorization_code>`, `redirect_uri={issuer}/deviceauth/callback`, `code_verifier=<the server's>`. Reply `{id_token, access_token, refresh_token}` stored through the shared mapping (OpenAI keeps `id_token`, which the Codex client reads `chatgpt_account_id` from). rupu's browser flow does no API-key token exchange, and neither does this.

### D4. Test seams

Every endpoint a flow calls has an override, named like the existing ones and read only by the flow: `RUPU_DEVICE_DEVICE_URL_OVERRIDE` / `RUPU_DEVICE_TOKEN_URL_OVERRIDE` (every RFC 8628 flow, as today) and `RUPU_DEVICE_FAST_POLL`; `RUPU_OAUTH_TOKEN_URL_OVERRIDE` (the paste flows' exchange, as the browser flow); `RUPU_OPENAI_DEVICE_ISSUER_OVERRIDE` (the OpenAI issuer, so the two accounts endpoints and the exchange all point at the mock). A chosen GitLab application's endpoints are never overridden (as today): the two URL-override seams (`RUPU_DEVICE_DEVICE_URL_OVERRIDE` / `RUPU_DEVICE_TOKEN_URL_OVERRIDE`) never apply to it, while `RUPU_DEVICE_FAST_POLL` — a sleep seam, not an endpoint — applies to a chosen GitLab application too. The OpenAI flow's in-process tests poll with no wait through `openai_device::Options.interval_override` (`Options::from_env` sets none). The pasted code comes from stdin.

### D5. Messages

One module-level refusal per situation, each naming the next step. The `callback.rs` headless refusal changes to name `--mode device`. The `AuthModeArg` help text lists `device` as "log in from a browser on another machine".

## Testing

- `rupu-auth` `tests/it`: one httpmock test per flow and per notable branch — RFC 8628 success for GitLab with a chosen application (extras recorded), `slow_down` without `interval` adds 5 s (fast-poll seam), `expired_token` ends, 404 on `authorize_device` gives the version message; OpenAI usercode → pending (403, 404) → success → exchange with the server's verifier and the `deviceauth/callback` redirect, 404 on usercode gives the not-enabled message, 15-minute deadline (clock seam or a fast-poll seam with a tiny budget); paste flow for Anthropic with `code#state`, a full URL, a query string, a bare code, and a mismatched state refused before any exchange; paste flow for Gemini with a bare code, and a reply without `refresh_token` refused with nothing stored.
- `rupu-cli` `tests/it/auth_login_modes.rs`: `--mode device` with the vendor mocked stores an `sso` credential and `auth status` shows it; `--mode sso` on headless Linux names `--mode device`; `--mode device` on an `openai-compatible` provider refuses; the pasted code is read from a non-TTY stdin.
- `auth logout --mode device` removes the `sso` entry.
- Live: matt runs each of OpenAI, Anthropic and GitLab on the Linux VM before the beta; the design cannot be verified against the vendors from CI. Gemini is live-run too when matt has a Google account at hand; a Gemini flow that ships without a live run is marked "not yet verified against Google" in `docs/providers.md` until one happens. Two wire facts there rest on reference code alone: the hosted `codeassist.google.com/authcode` page as the `redirect_uri` on both the authorize request and the exchange, and the consent parameters above.

## Left to the builder

- Module and function names, the shape of `HeadlessFlow`, how the three flows share the authorize-URL builder and the token mapping (a `token.rs` is the suggestion, not a requirement).
- Exact prompt wording within the messages given above.
- Whether the OpenAI 15-minute budget is a clock seam or a fast-poll seam in tests.

## Appendix: research notes (2026-11-08)

Sources read, by vendor. The wire tables in the design are taken from these.

- **OpenAI**: `openai/codex` `codex-rs/login/src/device_code_auth.rs`, `server.rs`, `oauth/client.rs`, `token_data.rs`, `auth/manager.rs`, `auth/default_client.rs`, `cli/src/login.rs`; tests `login/tests/suite/device_code_login.rs`, `cli/tests/login.rs`; docs `learn.chatgpt.com/docs/auth` (device login is beta, enabled per account or workspace). Codex has a second, RFC 8628 implementation (`oauth/device.rs`) for enterprise gateway IdPs; it is not the ChatGPT path and is not used here.
- **Anthropic**: `ex-machina-co/opencode-anthropic-auth` (`src/constants.ts`, `src/auth.ts`), `NousResearch/hermes-agent` `agent/anthropic_credentials.py`, `helixml/helix` `api/pkg/anthropic/oauth.go`, `1jehuang/jcode` `crates/jcode-base/src/auth/oauth.rs`, pi-ai `dist/utils/oauth/anthropic.js`; Claude Code docs `code.claude.com/docs/en/authentication` ("If your browser shows a login code instead of redirecting back… paste it"); Claude Code issues #22992 (device flow requested, unanswered), #88877 (the registered redirect path is exactly `/oauth/code/callback`). Token endpoint `platform.claude.com/v1/oauth/token` (rupu's current one); the legacy `console.anthropic.com` redirect redirects to the platform page and is accepted when pasted as a URL.
- **Gemini**: `google-gemini/gemini-cli` `packages/core/src/code_assist/oauth2.ts` (`authWithUserCode`: `redirect_uri = https://codeassist.google.com/authcode`, PKCE S256, `access_type=offline`, 2 attempts, TTY required), `utils/browser.ts`, `code_assist/setup.ts`; `google-auth-library-nodejs` `oauth2client.ts` for what goes on the wire; Google's OOB migration page (OOB blocked since 2023-01-31) and limited-input-device page (allowed scopes exclude `cloud-platform`); a live probe of `oauth2.googleapis.com/device/code` with the Gemini CLI client id answered `invalid_client: Invalid client type`.
- **GitLab**: `docs.gitlab.com/api/oauth2/` (device authorization grant, GA 17.9, flag `oauth2_device_grant_flow` from 17.2), `gitlab-org/cli` `internal/oauth2/device.go`, `internal/commands/auth/login/login.go` (`--device`, same gitlab.com client id, "Requires GitLab 17.9 or later"), `gitlab-org/api/client-go` `gitlaboauth2/config.go` (`DeviceAuthURL = {base}/oauth/authorize_device`), `golang/oauth2` `deviceauth.go` (`slow_down` adds 5 s), GitLab `config/initializers/doorkeeper.rb` (`grant_flows` includes `device_code` instance-wide; refresh tokens on for every grant), the `doorkeeper-device_authorization_grant` gem (errors, 300 s / 5 s defaults). Known server bug: a self-managed instance under a relative URL root returned a `verification_uri` without the prefix (GitLab MR !248460, fixed Aug 2026); rupu shows the server's URI and the code, so the operator can still type it on the right page.

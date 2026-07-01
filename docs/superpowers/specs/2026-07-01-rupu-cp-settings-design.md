# rupu CP — Settings & Project Config (with policy enforcement)

Status: approved (design), pending implementation plan
Date: 2026-07-01

## Context

The CP's `Settings.tsx` is a stub ("Settings coming soon.") with a live
`/settings` nav slot. There is no config read/write API today. Config is
resolved by `rupu_config::layer_files(global config.toml, project config.toml)`
into a typed `Config` (`default_provider/model`, `permission_mode`, `log_level`,
`ui`, `bash`, `retry`, `scm`, `issues`, `pricing`, `autoflow`, `storage`,
`triggers`), layered **global → project → env** where project overrides global.

The CP is read-mostly (per the record-in-CP / write-in-`cp serve` split): a few
mutations exist (host-add is launcher-gated; approvals are markers; the resume
worker lives in the full `cp serve` runtime). This feature adds a **config
write** surface, so it must fit that split — writes are gated to the full
`cp serve` runtime, validated, atomic, and backed up.

This spec delivers, in v1: a **global settings** editor, a **per-project config**
editor, both **read-write**; a real **policy-lock enforcement** layer; and a
**hybrid** (typed form + raw TOML) editing model.

## Spine decisions (approved)

1. **Both surfaces, read-write, v1.** A global Settings page (fills the stub) and
   a per-project Config editor (a tab on the existing `ProjectDetail` page).
2. **Real policy enforcement via a `[policy] lock` layer in `rupu-config`.** A
   locked global key's value overrides project + env — per-key precedence becomes
   `locked-global > env > project > global > default`. The lock lives in the
   **core** so it is enforced everywhere config is read (CLI, agent runs, cron,
   cp serve), not only in the CP. The CP manages and surfaces it.
3. **Hybrid editing:** each surface has a **Form** tab (typed fields from the
   schema, per-field validation, inline provenance + 🔒 lock toggle, secrets
   masked) and a **Raw TOML** tab (syntax-highlighted, validate-on-save). Both
   write the same validated file through the same API.
4. **CP-runtime settings get a new `[cp]` config section** (approved): persistable
   runtime settings such as `max_workspace_bytes` (promoting today's const),
   transport enablement/policy, approval-policy defaults.
5. **`bind` / `token` are display-only** (approved): changing them needs a
   restart, so they are shown read-only (token masked as `set`), with a
   "requires restart" note — never live-edited.

## Goals

- View the **effective** config with per-key **provenance** (which layer won) and
  **lock** state, for both global and each project.
- **Edit and persist** global config, the `[cp]` runtime section, and a project's
  `.rupu/config.toml` — via form or raw TOML — with validation, backup, and
  atomic writes.
- **Enforce** policy: mark global keys as locked so projects/env cannot override
  them; the lock takes effect wherever config is resolved.
- Secrets are never displayed or written here (keychain-managed; shown as
  configured/not-configured); `token` masked.
- No silent-noop: a successful write persists to disk and the CP reloads its
  in-memory snapshot for new runs; settings that require a restart to take
  effect are flagged as such.

## Non-goals (v1 / later)

- Hot-reload of restart-required settings (`bind`/`token`) — flagged, not live.
- Editing agents/workflows **definitions** from the CP (the Build viewer's write
  story is separate).
- Multi-user auth / RBAC beyond the existing bearer token.
- Editing provider **secrets** in the settings UI (keychain via the existing auth
  flow).

## Architecture

### 1. `rupu-config` — policy-lock engine + provenance

- **`[policy]` block** in the *global* config: `lock = ["<dotted.key.path>", …]`
  (e.g. `["permission_mode", "autoflow.max_active", "cp.max_workspace_bytes"]`).
  Locked keys are only honored from the **global** layer.
- **Resolution change:** extend the layering so, for a locked key, the global
  value overrides project + env. Non-locked keys keep today's `env > project >
  global` precedence. New API: `resolve(global, project, env) -> Resolved` where
  `Resolved` exposes the effective `Config` **plus** a provenance map
  `key -> { value, source: Global|Project|Env|Default, locked: bool }`. The
  existing `layer_files` remains (or is re-expressed in terms of `resolve`) so
  current callers are unaffected when there is no `[policy]` block.
- The lock is a `rupu-config` concern; **every** config consumer gets enforcement
  for free.
- `[cp]` section type added to `Config` (`CpConfig { max_workspace_bytes:
  Option<u64>, … }`), defaulted so absence is a no-op; `rupu-cp` reads it where it
  currently uses the `MAX_WORKSPACE_BYTES` const (falling back to the const
  default).

### 2. `cp serve` — config read/write API (`crates/rupu-cp/src/api/config.rs`)

- **`GET /api/config`** (+ `?project=<id>`): returns the effective resolved
  config, the per-key provenance/lock map, the raw global TOML and (when a
  project is given) the raw project TOML, the `[cp]` runtime settings, and
  display-only runtime status (`bind`, `token` masked, process info).
- **`PUT /api/config/global`** and **`PUT /api/config/project/<id>`**: write the
  respective `config.toml`. Body is the full raw TOML (from the raw editor) **or**
  a structured patch (from the form) — both are materialized to candidate TOML,
  then run through the **write-path safety contract** (below).
- **`PUT /api/config/policy`**: set the global `[policy] lock` list.
- Writes are **gated to the full `cp serve` runtime** and the same
  launcher/token gate host-add uses. On success the CP **reloads** its in-memory
  config snapshot (AppState `Config`/`PricingConfig`/`[cp]`), so new runs use the
  new values; a response flag marks any setting that needs a restart.
- Reuse `api/fs_safety.rs` confinement for the project config path (the file must
  resolve under the project's `.rupu/`).

### 3. Write-path safety contract

Every write: **validate** (parse candidate TOML into the typed `rupu_config`
schema; reject unknown keys and type errors — surfaced per-field for the form,
per-line for raw) → **backup** the prior file to `config.toml.bak` → **atomic**
write (temp file + rename) → advisory **file lock** to serialize concurrent
writers. A rejected write leaves the on-disk file untouched.

### 4. CP web

- **Global** (`Settings.tsx`, fills the stub): sub-tabs *General · Providers ·
  Autoflow · SCM/Issues · Pricing · CP-Runtime · Policy · Raw*. Form tab renders
  typed fields with provenance badges + a 🔒 lock toggle per key (writing the
  `[policy]` list); Raw tab is the highlighted TOML with validate-on-save;
  display-only runtime status panel (bind/token-masked/restart notes).
- **Per-project** (a **Config** tab on `ProjectDetail.tsx`): Form + Raw for that
  project's `.rupu/config.toml`, with provenance showing values inherited or
  **locked** from global (locked keys are read-only here with a 🔒 + "enforced by
  global" note).
- A shared `api.ts` client for the config endpoints; secret fields render as
  `••• set` / "not configured" and are never populated from the server.

## Errors & security

- Validation failure → the write is rejected with a precise message (field or
  TOML line); the file is unchanged.
- A project attempt to set a **locked** key is rejected (or shown read-only) with
  a clear "enforced by global policy" message.
- Writes require the full `cp serve` runtime + the existing gate; the read-only
  `cp` surface cannot write.
- Project config path confined under the project's `.rupu/` (`fs_safety`); no
  traversal.
- Secrets never displayed or written; `token` masked; provider keys stay in the
  keychain.
- Atomic write + backup: no corrupt/partial config file on failure.
- `#![deny(clippy::all)]`; no `unsafe`; library errors `thiserror`, CLI/API
  `anyhow`/`ApiError`; workspace deps only.

## Testing

- **`rupu-config`:** policy-lock resolution (locked global overrides project/env;
  unlocked keeps `env > project > global`); provenance map correctness; `[cp]`
  section parse + default; backward compatibility (no `[policy]`/`[cp]` block ⇒
  today's behavior byte-for-byte).
- **write-path safety:** validate rejects unknown-key/type errors without
  touching the file; backup created; atomic (no partial file on simulated
  failure); concurrent writes serialized; project path confinement rejects
  traversal; secret fields never echoed in `GET`.
- **API:** `GET` returns effective config + provenance + raw + `[cp]` + masked
  status; `PUT global`/`PUT project`/`PUT policy` persist and reload the snapshot;
  writes gated (read-only surface refused); locked-key project write rejected.
- **web:** form validation + lock toggle; raw validate-on-save rejects bad TOML;
  provenance/lock badges render; secrets masked; project Config tab shows
  inherited/locked state.
- **e2e:** edit a value (form and raw) → persist → `GET` reflects it → a new run
  resolves the new value; lock a key globally → project override is ignored at
  resolution.

## Open questions

- **Form ↔ raw source of truth on save:** whether the form submits a structured
  patch merged server-side into the existing TOML (preserving comments/unknown
  keys) or re-serializes the whole typed `Config` (dropping comments). Resolve in
  the plan; **prefer** a comment-preserving edit (e.g. `toml_edit`) so raw edits
  and hand-written comments survive form saves — added as a workspace dep if
  needed.
- **`[cp]` section breadth for v1:** start with `max_workspace_bytes` +
  approval-policy default; add transport-enablement toggles only if cheap.
  Resolve in the plan.
- **Which config keys are lockable:** all typed keys by dotted path vs a
  curated policy-relevant subset. Resolve in the plan; **prefer** allowing any
  typed key path, validated against the schema.

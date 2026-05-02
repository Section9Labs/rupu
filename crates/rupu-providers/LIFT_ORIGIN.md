# Lift origin

This crate was lifted from `Section9Labs/phi-cell` `origin/main` at commit
`1bfa105b2afb84a6e16a0f6412b03ea849425a33` on 2026-05-01.

## What was changed

- Renamed the package from `phi-providers` to `rupu-providers` in `Cargo.toml`.
- Adapted dependencies to rupu's workspace: `fs2.workspace = true` and
  `tempfile.workspace = true` (was `fs2 = "0.4"` and `tempfile = "3"`).
- Added `rust-version.workspace = true` to `[package]` (MSRV 1.77).
- Added `[lints] workspace = true` for clippy + unsafe_code inheritance.
- Replaced internal `phi_providers` / `phi-providers` references with
  `rupu_providers` / `rupu-providers` (4 occurrences across 3 files:
  `tests/integration.rs` ×2, `src/broker_types.rs` ×1, `src/auth/mod.rs` ×1).

## Workspace Cargo.toml dep changes

None required. rupu's workspace already had `reqwest` with `["json", "stream",
"rustls-tls", "http2", "charset"]` — the lifted code does not use `multipart`
or `native-tls`. The `ed25519-dalek` features `["serde", "rand_core", "zeroize"]`
are sufficient; the lifted code uses only `SigningKey` / `Signer`, which do not
require `pkcs8` or `pem`.

## Why this is a hard lift, not a fork

We do not plan to re-sync from upstream phi-cell. Once lifted, this crate
evolves independently. If phi-cell's provider stack gets a meaningful
improvement we want to bring back, port it as a deliberate change with its
own commit and PR — not a merge.

## How to refresh from upstream (manual)

If you ever need to bring in newer phi-cell work, do NOT do a tree-merge.
Instead, look at specific files in `Section9Labs/phi-cell` `origin/main`
and port the changes manually with their own commit message documenting
what was brought in and why.

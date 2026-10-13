# Skills on placed units, Plan 1: format, cache, overlay, SSH and HTTP

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A placed unit on an SSH or HTTP host runs with the coordinator's copies of its skills, shipped with the dispatch and cached on the host by content hash.

**Architecture:**
- **`rupu-skills`** gains a `ship` module: pack and unpack, the host cache, the bundle manifest and the stdin stream framing. It also gains an overlay on `LayerDirs` that the catalog merges over the installed layers.
- **The host:** `rupu run --skill-bundle=<json>` turns the manifest into that overlay, for both the agent load and the assembler.
- **The coordinator:** the fleet unit dispatcher resolves the unit's skills and asks the connector to ship them, through a new required `HostConnector::ship_skills`. The connector answers shipped, not needed (local) or unsupported, which is the P5 downgrade that becomes a `StepWarning`.
- **Transports:** SSH uses two hidden subcommands (`rupu __skills missing|receive`), and HTTP uses two endpoints.

**Tech Stack:** Rust (tokio, axum 0.7, `tar` 0.4, serde), TypeScript for the one web renderer line.

**Spec:** `docs/superpowers/specs/2026-10-13-rupu-skills-on-placed-units-design.md` (decisions P1–P6). Read it first. The Agent Skills spec it builds on is `docs/superpowers/specs/2026-10-12-rupu-agent-skills-design.md` (§9).

## Global Constraints

- Workspace deps only. `tar` is already pinned in the root `Cargo.toml` as `tar = "0.4"`, so add `tar = { workspace = true }` to `crates/rupu-skills/Cargo.toml`. Never pin a version in a crate's `Cargo.toml`.
- `rupu-skills` stays a leaf crate with no rupu dependencies.
- Caps (P6): `MAX_SHIPPED_SKILL_BYTES = 32 MiB` per skill tree; `MAX_BUNDLE_BYTES = 128 MiB` per unit.
- The capability string is `agent.skill_bundle` (`CAP_AGENT_SKILL_BUNDLE`). Plan 1 adds it to `host_features()` only, which covers HTTP `/api/host/info` and SSH `rupu __features`. Tunnel and bucket are Plan 2. Until then their `ship_skills` answers `Unsupported`, which is the P5 downgrade.
- The host cache is `<RUPU_HOME>/cache/skills/<content_hash hex>/<skill name>/`, with the directory created 0700 on unix.
- A shipped skill overlays the host's installed skill with the same (layer, name) (P2). The S9 rules are unchanged: `*` covers the global layer only, and a bare name bound in both layers is ambiguous.
- `host: local` ships nothing.
- Integration tests are modules under `crates/<c>/tests/it/` (one binary per crate). `rupu-cli` env- and cwd-mutating tests go under `tests/serial/` and hold `ENV_LOCK`. Run serial tests with `< /dev/null`.
- Never run bare `git stash` / `git stash pop`. Push with an explicit refspec. Never run package-wide `cargo fmt`; format only the files you touched (`rustfmt --edition 2021 <file>`).
- `#![deny(clippy::all)]` is on workspace-wide, so `cargo clippy -p <crate> --all-targets` must be clean for every crate you touch.

## Deviations from the spec (decided here, flagged for review)

1. **The manifest rides inline: `--skill-bundle=<json>`, not a file.** It is a single argv element (a few hundred bytes for a typical bundle), so no temp file has to be staged on the host first. The run still copies it into `<RUPU_HOME>/runs/<run_id>/skill_bundle.json` for `--continue`.
2. **SSH makes two commands per batch, not one.** `RemoteExec::run_bytes` writes all of stdin and then waits for output (`crates/rupu-cp/src/host/ssh.rs:990`), so a back-and-forth exchange over one connection would need a new duplex exec. The first command, `rupu __skills missing`, is gated on the feature and lists the hashes the host lacks. The second, `rupu __skills receive`, takes those tars on stdin. That is two connections per host per process, because of the memo, not per unit. That keeps the no-burst property.
3. **Single-flight is a per-connector ship lock**, not a lock per hash. Concurrent units for one host wait for the first ship to finish and then find their hashes confirmed. Connectors are cached per host by `HostRegistry`, so a per-instance memo is per host.
4. **Host-id prefix.** Every error the dispatcher produces for a placed unit, both `Err(UnitFailure)` and `UnitOutcome.error`, is prefixed `host <id>: ` unless it already starts with that. This covers the spec §5 fix, including a skill missing on the host, which reaches the coordinator as the remote run's error.

## Review Focus

1. **Two units store the same hash on one host at once.** Both succeed and one copy is kept. A race on the final rename is not an error (Task 2).
2. **A skill containing an absolute symlink, even one that resolves inside its root.** `walk_tree` accepts it (`crates/rupu-skills/src/tree.rs:85`), but it would point somewhere else on the host. Pack refuses it, and the message names the link (Task 1).
3. **A crash in the middle of unpacking.** Only a renamed-in directory counts as cached. Leftover temp directories are never reported as present, and a later store still works (Task 2).
4. **An SSH remote with an old rupu (no `__skills`, no `agent.skill_bundle`).** It gets the P5 downgrade, and the unit still launches by name with a `StepWarning` naming the host (Task 7).
5. **The coordinator has no copy of the unit's agent.** Only the step's skills ship, and the host resolves the agent's own grants from its installed layers (Task 8).

---

### Task 1: Pack and unpack a skill

**Files:**
- Modify: `crates/rupu-skills/Cargo.toml` (add `tar = { workspace = true }`)
- Modify: `crates/rupu-skills/src/tree.rs` (add `ContentHasher`; make `content_hash` use it)
- Create: `crates/rupu-skills/src/ship.rs`; modify `crates/rupu-skills/src/lib.rs` (`pub mod ship;`)
- Test: `crates/rupu-skills/tests/it/ship.rs` (add `mod ship;` to `tests/it/main.rs`)

**Interfaces:**
- Produces, in `rupu_skills::tree`:
  ```rust
  pub struct ContentHasher(/* sha2::Sha256 */);
  impl ContentHasher {
      pub fn new() -> Self;
      pub fn file(&mut self, rel: &str, mode: u32, bytes: &[u8]);
      pub fn symlink(&mut self, rel: &str, target: &str);
      pub fn finish(self) -> ContentHash;
  }
  ```
- Produces, in `rupu_skills::ship`:
  ```rust
  pub const MAX_SHIPPED_SKILL_BYTES: u64 = 32 << 20;
  pub const MAX_BUNDLE_BYTES: u64 = 128 << 20;
  pub struct PackedSkill { pub layer: Layer, pub name: String, pub hash: ContentHash, pub bytes: Vec<u8> }
  pub fn pack_skill(skill: &Skill) -> Result<PackedSkill, ShipError>;
  pub fn unpack_skill(bytes: &[u8], dest: &Path, expected: &ContentHash) -> Result<(), ShipError>;
  #[derive(Debug, thiserror::Error)]
  pub enum ShipError {
      #[error("skill {name}: {source}")] Tree { name: String, source: TreeError },
      #[error("skill {name}: symlink {rel} has an absolute target, which can't be shipped")] AbsoluteSymlink { name: String, rel: String },
      #[error("skill {name} is {bytes} bytes, over the {limit}-byte limit for a shipped skill")] TooLarge { name: String, bytes: u64, limit: u64 },
      #[error("the skills to ship total {bytes} bytes, over the {limit}-byte limit for one unit")] BundleTooLarge { bytes: u64, limit: u64 },
      #[error("shipped skill archive: {0}")] Malformed(String),
      #[error("shipped skill content hash is {got}, expected {expected}")] HashMismatch { expected: String, got: String },
      #[error("{path}: {source}")] Io { path: PathBuf, source: std::io::Error },
      #[error("shipped skill {layer}:{name} ({hash}) is not in this host's skill cache")] NotCached { layer: Layer, name: String, hash: String },
  }
  ```
  Display `Layer` with `as_str()`, and add `impl std::fmt::Display for Layer` in `skill.rs` if it is missing.

- [ ] **Step 1: Write failing tests** in `tests/it/ship.rs`. Use `crate::support::write_skill` to build skills in a tempdir, then load them with `rupu_skills::load_skill(dir, Layer::Global)`.

```rust
use rupu_skills::ship::{pack_skill, unpack_skill, ShipError, MAX_SHIPPED_SKILL_BYTES};
use rupu_skills::tree::{content_hash, walk_tree, ContentHasher};
use rupu_skills::{load_skill, Layer};

#[test]
fn hasher_matches_content_hash() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = crate::support::write_skill(tmp.path(), "pdf", "Read PDFs.", "body");
    std::fs::create_dir_all(dir.join("scripts")).unwrap();
    std::fs::write(dir.join("scripts/run.sh"), b"#!/bin/sh\necho hi\n").unwrap();
    let entries = walk_tree(&dir).unwrap();
    let mut h = ContentHasher::new();
    for e in &entries {
        match &e.kind {
            rupu_skills::tree::EntryKind::File { mode, .. } => {
                h.file(&e.rel, *mode, &std::fs::read(dir.join(&e.rel)).unwrap())
            }
            rupu_skills::tree::EntryKind::Symlink { target } => h.symlink(&e.rel, target),
        }
    }
    assert_eq!(h.finish(), content_hash(&dir, &entries).unwrap());
}

#[test]
fn pack_then_unpack_reproduces_the_content_hash() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = crate::support::write_skill(tmp.path(), "pdf", "Read PDFs.", "body");
    std::fs::create_dir_all(dir.join("references")).unwrap();
    std::fs::write(dir.join("references/REF.md"), b"ref").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink("references/REF.md", dir.join("LINK.md")).unwrap();
    let skill = load_skill(&dir, Layer::Global).unwrap();
    let packed = pack_skill(&skill).unwrap();
    assert_eq!(packed.hash, skill.content_hash().unwrap());
    let out = tempfile::tempdir().unwrap();
    let dest = out.path().join("pdf");
    unpack_skill(&packed.bytes, &dest, &packed.hash).unwrap();
    let entries = walk_tree(&dest).unwrap();
    assert_eq!(content_hash(&dest, &entries).unwrap(), packed.hash);
    assert_eq!(load_skill(&dest, Layer::Global).unwrap().body, "body");
}

#[cfg(unix)]
#[test]
fn an_absolute_symlink_is_refused_even_inside_the_root() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = crate::support::write_skill(tmp.path(), "pdf", "Read PDFs.", "body");
    std::fs::write(dir.join("a.md"), b"a").unwrap();
    let abs = dir.canonicalize().unwrap().join("a.md");
    std::os::unix::fs::symlink(&abs, dir.join("b.md")).unwrap();
    let skill = load_skill(&dir, Layer::Global).unwrap();
    let err = pack_skill(&skill).unwrap_err();
    assert!(matches!(&err, ShipError::AbsoluteSymlink { rel, .. } if rel == "b.md"), "{err}");
}

#[test]
fn a_tampered_archive_fails_the_hash_check_and_leaves_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = crate::support::write_skill(tmp.path(), "pdf", "Read PDFs.", "body");
    let skill = load_skill(&dir, Layer::Global).unwrap();
    let packed = pack_skill(&skill).unwrap();
    let wrong = rupu_skills::ContentHash([7u8; 32]);
    let out = tempfile::tempdir().unwrap();
    let dest = out.path().join("pdf");
    let err = unpack_skill(&packed.bytes, &dest, &wrong).unwrap_err();
    assert!(matches!(err, ShipError::HashMismatch { .. }), "{err}");
    assert!(!dest.exists());
}

/// Hand-built archives the coordinator would never produce.
fn tar_with(path: &str, kind: tar::EntryType, link: Option<&str>, data: &[u8]) -> Vec<u8> {
    let mut b = tar::Builder::new(Vec::new());
    let mut h = tar::Header::new_ustar();
    h.set_entry_type(kind);
    h.set_mode(0o644);
    h.set_size(data.len() as u64);
    if let Some(l) = link {
        h.set_link_name(l).unwrap();
    }
    // set_path refuses "..", so write the raw name bytes.
    let name = &mut h.as_old_mut().name;
    name[..path.len()].copy_from_slice(path.as_bytes());
    h.set_cksum();
    b.append(&h, data).unwrap();
    b.into_inner().unwrap()
}

#[test]
fn unpack_refuses_escaping_paths_links_and_devices() {
    let any = rupu_skills::ContentHash([0u8; 32]);
    for (bytes, what) in [
        (tar_with("../evil", tar::EntryType::Regular, None, b"x"), "dot-dot"),
        (tar_with("/etc/evil", tar::EntryType::Regular, None, b"x"), "absolute"),
        (tar_with("l", tar::EntryType::Symlink, Some("../../x"), b""), "escaping link"),
        (tar_with("l", tar::EntryType::Symlink, Some("/etc/passwd"), b""), "absolute link"),
        (tar_with("d", tar::EntryType::Char, None, b""), "device"),
    ] {
        let out = tempfile::tempdir().unwrap();
        let dest = out.path().join("pdf");
        let err = unpack_skill(&bytes, &dest, &any).unwrap_err();
        assert!(matches!(err, ShipError::Malformed(_)), "{what}: {err}");
        assert!(!dest.exists(), "{what}");
        assert!(!out.path().join("evil").exists() && !out.path().parent().unwrap().join("evil").exists());
    }
}

#[test]
fn an_over_cap_skill_is_refused_by_pack() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = crate::support::write_skill(tmp.path(), "big", "Big.", "body");
    let f = std::fs::File::create(dir.join("blob.bin")).unwrap();
    f.set_len(MAX_SHIPPED_SKILL_BYTES + 1).unwrap(); // sparse; cheap
    let skill = load_skill(&dir, Layer::Global).unwrap();
    assert!(matches!(pack_skill(&skill).unwrap_err(), ShipError::TooLarge { .. }));
}
```

  If `support::write_skill` takes different arguments, read `crates/rupu-skills/tests/it/support.rs` and adapt the calls. The behaviours asserted stay the same.

- [ ] **Step 2: Run them and confirm they fail.** Run `cargo test -p rupu-skills --test it ship::`. Expected: compile errors, because `ship` and `ContentHasher` don't exist yet.

- [ ] **Step 3: Implement `ContentHasher`** in `tree.rs`. It must feed bytes exactly as `content_hash` does:
  - for a file: `rel`, `0`, then `f\0{mode:o}\0{len}\0`, the bytes, then `\n`;
  - for a symlink: `rel`, `0`, `l\0`, the target, then `\n`.

  Rewrite `content_hash` to read each file and call `ContentHasher::file`, or keep its streaming loop. Either way, `hasher_matches_content_hash` is the lockstep test.

- [ ] **Step 4: Implement `ship.rs`.**
  - **`pack_skill`:**
    1. `walk_tree(&skill.root)` fresh, so the pack reflects disk now.
    2. Refuse any `Symlink { target }` where `Path::new(target).is_absolute()` → `AbsoluteSymlink`.
    3. Sum the file `len`s and refuse over `MAX_SHIPPED_SKILL_BYTES` → `TooLarge`, before reading any bytes.
    4. Build a `tar::Builder<Vec<u8>>`. For each entry in walk order: read the file's bytes once, feed them to the `ContentHasher`, then append a ustar header (`EntryType::Regular`, `set_mode(mode)`, `set_mtime(0)`, uid/gid 0, `set_size(bytes.len())`, path `rel`) with those same bytes. For a symlink, use `EntryType::Symlink`, `set_link_name(target)`, size 0, and `hasher.symlink`.
    5. The hash returned is `hasher.finish()`, the hash of exactly the bytes in the archive.
  - **`unpack_skill`:**
    1. Refuse when `dest` exists.
    2. Create `dest`.
    3. Iterate `tar::Archive::new(bytes).entries()`. For each entry, take its path from `entry.path()`. Refuse it as `Malformed` if it is absolute, or if any component isn't `Component::Normal`.
    4. Allow only `Regular`, `Directory` and `Symlink` entries.
    5. For a symlink, `link_name` must be relative. Resolve `parent(rel).join(target)` lexically, popping on `..` and refusing a pop past the root, and require the result to stay inside.
    6. Track a running total of `header.size()` and refuse past `MAX_SHIPPED_SKILL_BYTES`.
    7. Write files with `std::fs::write` after `create_dir_all(parent)`, and set the mode to `0o755` if the header mode has any exec bit, else `0o644` (unix `PermissionsExt`). Create symlinks with `std::os::unix::fs::symlink`. On non-unix, refuse a symlink as `Malformed`.
    8. After the loop, `walk_tree(dest)` and `content_hash`. If the result differs from `expected`, return `HashMismatch`.
    9. On ANY error, `remove_dir_all(dest)` before returning.

- [ ] **Step 5: Run the tests.** Run `cargo test -p rupu-skills --test it ship::` and `cargo test -p rupu-skills`. All must pass, including the existing hash tests.

- [ ] **Step 6: Clippy and commit.** Run `cargo clippy -p rupu-skills --all-targets`, then:
  ```bash
  git add crates/rupu-skills
  git commit -m "feat(skills): pack and unpack a skill for shipping"
  ```

---

### Task 2: Host cache, bundle manifest and stdin stream

**Files:**
- Modify: `crates/rupu-skills/src/ship.rs`
- Test: `crates/rupu-skills/tests/it/ship.rs`

**Interfaces:**
- Consumes: `pack_skill`, `unpack_skill`, `ShipError` (Task 1).
- Produces, in `rupu_skills::ship`:
  ```rust
  #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
  pub struct ShippedSkillRef { pub layer: Layer, pub name: String, pub content_hash: String /* 64 hex */ }
  #[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
  pub struct SkillBundleManifest { pub version: u32 /* 1 */, pub skills: Vec<ShippedSkillRef> }
  impl SkillBundleManifest {
      pub const VERSION: u32 = 1;
      /// Every name is a valid `SkillName`, every hash 64 lowercase hex, no (layer, name) twice, version == 1.
      pub fn validate(&self) -> Result<(), ShipError>;
  }
  impl PackedSkill { pub fn reference(&self) -> ShippedSkillRef; }
  pub fn cache_dir(global: &Path) -> PathBuf;                       // global.join("cache").join("skills")
  pub fn cached_path(cache: &Path, r: &ShippedSkillRef) -> PathBuf; // cache/<hash>/<name>
  pub fn missing(cache: &Path, refs: &[ShippedSkillRef]) -> Vec<ShippedSkillRef>; // whose cached_path/SKILL.md isn't a file
  pub fn store(cache: &Path, r: &ShippedSkillRef, bytes: &[u8]) -> Result<PathBuf, ShipError>;
  pub fn overlay_for(cache: &Path, m: &SkillBundleManifest) -> Result<Vec<crate::catalog::OverlaySkill>, ShipError>; // NotCached on a gap
  /// stdin framing for `rupu __skills receive`: per skill, a line "<hash> <layer> <name> <len>\n" then <len> bytes.
  pub fn encode_stream(skills: &[(&ShippedSkillRef, &[u8])]) -> Vec<u8>;
  pub fn decode_stream(r: &mut impl std::io::BufRead, total_cap: u64) -> Result<Vec<(ShippedSkillRef, Vec<u8>)>, ShipError>;
  ```
  Add `ShipError::Manifest(String)` for `validate` failures. `OverlaySkill` is defined in Task 3. Put `overlay_for` in this task anyway, so write Task 3's struct first if you run the tasks in order. If you run Task 2 alone, temporarily define `OverlaySkill` here and move it in Task 3.

- [ ] **Step 1: Write failing tests.**

```rust
use rupu_skills::ship::*;

fn packed(tmp: &std::path::Path, name: &str) -> PackedSkill {
    let dir = crate::support::write_skill(tmp, name, "D.", "body");
    pack_skill(&rupu_skills::load_skill(&dir, rupu_skills::Layer::Global).unwrap()).unwrap()
}

#[test]
fn store_then_missing_and_overlay() {
    let src = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let cache = cache_dir(home.path());
    let p = packed(src.path(), "pdf");
    let r = p.reference();
    assert_eq!(missing(&cache, std::slice::from_ref(&r)), vec![r.clone()]);
    let dir = store(&cache, &r, &p.bytes).unwrap();
    assert_eq!(dir, cached_path(&cache, &r));
    assert!(missing(&cache, std::slice::from_ref(&r)).is_empty());
    let m = SkillBundleManifest { version: 1, skills: vec![r.clone()] };
    let o = overlay_for(&cache, &m).unwrap();
    assert_eq!(o[0].dir, dir);
    // storing again is a no-op that returns the same dir
    assert_eq!(store(&cache, &r, &p.bytes).unwrap(), dir);
}

#[test]
fn concurrent_stores_of_one_hash_both_succeed() {
    let src = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let cache = cache_dir(home.path());
    let p = std::sync::Arc::new(packed(src.path(), "pdf"));
    let handles: Vec<_> = (0..8).map(|_| {
        let (p, cache) = (p.clone(), cache.clone());
        std::thread::spawn(move || store(&cache, &p.reference(), &p.bytes))
    }).collect();
    for h in handles { h.join().unwrap().unwrap(); }
    let hash_dirs: Vec<_> = std::fs::read_dir(&cache).unwrap()
        .filter_map(|e| e.ok()).filter(|e| !e.file_name().to_string_lossy().starts_with('.')).collect();
    assert_eq!(hash_dirs.len(), 1);
}

#[test]
fn a_leftover_temp_dir_is_not_cached_and_a_later_store_works() {
    let src = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let cache = cache_dir(home.path());
    let p = packed(src.path(), "pdf");
    std::fs::create_dir_all(cache.join(".tmp-crashed").join("pdf")).unwrap();
    assert_eq!(missing(&cache, &[p.reference()]).len(), 1);
    store(&cache, &p.reference(), &p.bytes).unwrap();
    assert!(missing(&cache, &[p.reference()]).is_empty());
}

#[test]
fn overlay_names_a_gap() {
    let home = tempfile::tempdir().unwrap();
    let m = SkillBundleManifest { version: 1, skills: vec![ShippedSkillRef {
        layer: rupu_skills::Layer::Global, name: "pdf".into(), content_hash: "a".repeat(64) }] };
    let err = overlay_for(&cache_dir(home.path()), &m).unwrap_err();
    assert!(err.to_string().contains("global:pdf"), "{err}");
}

#[test]
fn manifest_validation() {
    let ok = ShippedSkillRef { layer: rupu_skills::Layer::Global, name: "pdf".into(), content_hash: "a".repeat(64) };
    assert!(SkillBundleManifest { version: 1, skills: vec![ok.clone()] }.validate().is_ok());
    for bad in [
        SkillBundleManifest { version: 2, skills: vec![ok.clone()] },
        SkillBundleManifest { version: 1, skills: vec![ShippedSkillRef { name: "../x".into(), ..ok.clone() }] },
        SkillBundleManifest { version: 1, skills: vec![ShippedSkillRef { content_hash: "zz".into(), ..ok.clone() }] },
        SkillBundleManifest { version: 1, skills: vec![ok.clone(), ok.clone()] },
    ] {
        assert!(bad.validate().is_err(), "{bad:?}");
    }
}

#[test]
fn stream_round_trip_and_cap() {
    let src = tempfile::tempdir().unwrap();
    let (a, b) = (packed(src.path(), "pdf"), packed(src.path(), "notes"));
    let (ra, rb) = (a.reference(), b.reference());
    let bytes = encode_stream(&[(&ra, &a.bytes[..]), (&rb, &b.bytes[..])]);
    let got = decode_stream(&mut &bytes[..], MAX_BUNDLE_BYTES).unwrap();
    assert_eq!(got, vec![(ra.clone(), a.bytes.clone()), (rb, b.bytes.clone())]);
    assert!(decode_stream(&mut &bytes[..], 10).is_err());
    assert!(decode_stream(&mut &b"nonsense\n"[..], MAX_BUNDLE_BYTES).is_err());
}
```

- [ ] **Step 2: Run them and confirm they fail.** Run `cargo test -p rupu-skills --test it ship::`. Expected: compile errors.

- [ ] **Step 3: Implement.**
  - **`store`:**
    1. If `cached_path(r)/SKILL.md` is a file, return `cached_path`.
    2. `create_dir_all(cache)` and, on unix, set 0700.
    3. Create a temp dir in `cache` with a dot-prefixed name (`tempfile::Builder::new().prefix(".tmp-").tempdir_in(cache)`).
    4. `unpack_skill(bytes, &tmp.join(&r.name), &ContentHash::from_hex(&r.content_hash).ok_or(..)?)`.
    5. `std::fs::rename(tmp.path(), cache.join(&r.content_hash))`. If the rename fails and `cached_path/SKILL.md` now exists, another store won the race, so return Ok. Otherwise return `Io`. The `TempDir` drop cleans up the losing copy, so use `tmp.keep()` only on success.
  - **`missing`:** returns the refs whose `cached_path(r).join("SKILL.md")` isn't a file. `missing` and `overlay_for` ignore dot-prefixed directories by construction, because they only look up `<hash>/<name>`.
  - **`decode_stream`:**
    1. Read a header line of at most 512 bytes.
    2. Parse `hash layer name len`.
    3. Validate it as a `ShippedSkillRef`, using the same rules as `validate`.
    4. Refuse when the running total plus `len` exceeds `total_cap`.
    5. Read exactly `len` bytes.
    6. Stop at a clean EOF. A short read is `Malformed`.

- [ ] **Step 4: Run the tests.** Run `cargo test -p rupu-skills --test it ship::`. Expected: PASS.

- [ ] **Step 5: Clippy and commit.** `git commit -m "feat(skills): host cache, bundle manifest and receive stream"`

---

### Task 3: The overlay in `LayerDirs` and the catalog

**Files:**
- Modify: `crates/rupu-skills/src/catalog.rs`, `crates/rupu-skills/src/skill.rs` (`Skill.shipped`)
- Modify every `LayerDirs { .. }` struct literal so it still compiles. Add `overlay: Vec::new()` or `..Default::default()` at:
  - `crates/rupu-orchestrator/src/generate.rs:155,159`
  - `crates/rupu-skills/tests/it/catalog.rs` lines 70, 100, 117, 128, 146, 218, 311, 341, 393
  - `crates/rupu-skills/tests/it/grant.rs:15,246`

  Then grep `LayerDirs {` across `crates/` for any others.
- Modify every `Skill { .. }` literal, if any exist (grep `Skill {` in `crates/rupu-skills`).
- Test: `crates/rupu-skills/tests/it/catalog.rs`

**Interfaces:**
- Produces:
  ```rust
  // catalog.rs
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub struct OverlaySkill { pub layer: Layer, pub name: String, pub dir: PathBuf }
  pub struct LayerDirs { pub global: Option<PathBuf>, pub project: Option<PathBuf>, pub overlay: Vec<OverlaySkill> }
  impl LayerDirs { pub fn with_overlay(mut self, overlay: Vec<OverlaySkill>) -> Self; }
  // skill.rs
  pub struct Skill { /* existing */, pub shipped: bool }   // false from load_skill
  ```
  Re-export `OverlaySkill` from `lib.rs`.

- [ ] **Step 1: Write failing tests** in `tests/it/catalog.rs`.

```rust
use rupu_skills::{LayerDirs, OverlaySkill, SkillCatalog, Layer};

fn overlay_of(tmp: &std::path::Path, name: &str, desc: &str, layer: Layer) -> OverlaySkill {
    let dir = crate::support::write_skill(&tmp.join(format!("ov-{name}")), name, desc, "shipped body");
    OverlaySkill { layer, name: name.into(), dir }
}

#[test]
fn a_shipped_global_skill_replaces_the_installed_one() {
    let home = tempfile::tempdir().unwrap();
    crate::support::write_skill(&home.path().join("skills"), "pdf", "Installed.", "installed body");
    let dirs = LayerDirs::for_layers(home.path(), None)
        .with_overlay(vec![overlay_of(home.path(), "pdf", "Shipped.", Layer::Global)]);
    let cat = SkillCatalog::discover(&dirs).unwrap();
    let e = cat.get(Layer::Global, "pdf").unwrap();
    let s = e.skill().unwrap();
    assert_eq!(s.body, "shipped body");
    assert!(s.shipped);
    assert_eq!(cat.named("pdf").count(), 1);
}

#[test]
fn star_covers_overlay_global_but_not_overlay_project() {
    let home = tempfile::tempdir().unwrap();
    let dirs = LayerDirs::for_layers(home.path(), None).with_overlay(vec![
        overlay_of(home.path(), "a", "A.", Layer::Global),
        overlay_of(home.path(), "b", "B.", Layer::Project),
    ]);
    let cat = SkillCatalog::discover(&dirs).unwrap();
    let grants = rupu_skills::parse_grants(&["*".into()]).unwrap();
    let got = rupu_skills::resolve_grants(&grants, &cat).unwrap();
    let names: Vec<_> = got.iter().map(|s| s.name.as_str().to_string()).collect();
    assert_eq!(names, ["a"]);
}

#[test]
fn a_bare_name_bound_by_an_overlay_project_skill_and_an_installed_global_is_ambiguous() {
    let home = tempfile::tempdir().unwrap();
    crate::support::write_skill(&home.path().join("skills"), "pdf", "Installed.", "x");
    let dirs = LayerDirs::for_layers(home.path(), None)
        .with_overlay(vec![overlay_of(home.path(), "pdf", "P.", Layer::Project)]);
    let cat = SkillCatalog::discover(&dirs).unwrap();
    let grants = rupu_skills::parse_grants(&["pdf".into()]).unwrap();
    assert!(rupu_skills::resolve_grants(&grants, &cat).is_err());
}

#[test]
fn installed_skills_load_unshipped() {
    let home = tempfile::tempdir().unwrap();
    crate::support::write_skill(&home.path().join("skills"), "pdf", "Installed.", "x");
    let cat = SkillCatalog::discover(&LayerDirs::for_layers(home.path(), None)).unwrap();
    assert!(!cat.get(Layer::Global, "pdf").unwrap().skill().unwrap().shipped);
}
```

  If `parse_grants` takes `&[String]` as in `grant.rs:58`, these calls match. Adjust the `write_skill` signature to the helper's.

- [ ] **Step 2: Run them and confirm they fail.** Run `cargo test -p rupu-skills --test it catalog::`. Expected: compile errors (no `OverlaySkill`, `with_overlay` or `shipped`).

- [ ] **Step 3: Implement.**
  - **`CatalogEntry`** gains a private `shipped: bool`. `load()` maps the loaded skill so that `shipped` is set: `load_skill(&self.path, self.layer).map(|mut s| { s.shipped = self.shipped; Arc::new(s) })`.
  - **`discover_readable`:** after scanning the installed layers, remove every entry whose `(layer, dir_name)` matches an overlay item. Then push one entry per overlay item (`dir_name: name`, `path: dir`, `in_linked_root: false`, `shipped: true`) and re-sort by `(dir_name, layer)`.
  - **Overlay entries are never discovery errors.** A bad overlay directory surfaces as that entry's load error, the same as an installed one.
  - **`for_layers`** keeps `overlay` empty.

- [ ] **Step 4: Run the tests.** Run `cargo test -p rupu-skills` and `cargo test -p rupu-orchestrator --lib generate`, which compiles the literals. Expected: PASS.

- [ ] **Step 5: Clippy on rupu-skills and rupu-orchestrator, then commit.** `git commit -m "feat(skills): shipped skills overlay a run's installed layers"`

---

### Task 4: The run side: agent load, assembler, transcript `shipped`

**Files:**
- Modify: `crates/rupu-agent/src/loader.rs` (`load_agent_in`)
- Modify: `crates/rupu-runtime/src/assembly/spec.rs` (`Overrides.skill_overlay`) and `assembly/mod.rs` (`resolve_skills`; new pub `resolve_skill_requests`)
- Modify: `crates/rupu-transcript/src/event.rs` (`SkillIndexEntry.shipped`) and `skills.rs` (`skill_index_line`)
- Modify: `crates/rupu-agent/src/runner.rs:~3649` (`write_skill_index` sets `shipped`)
- Modify: `crates/rupu-cp/web/src/lib/transcript.ts:33`, `web/src/lib/transcriptLines.ts:30` and its test
- Modify the `SkillIndexEntry { .. }` literals so they compile:
  - `rupu-cli/src/cmd/session.rs:10393,10788`
  - `cmd/transcript.rs:2915`
  - `output/workflow_printer.rs:4973`
  - `rupu-runtime/tests/it/assembly_skills.rs:175,183`
  - `rupu-transcript/tests/it/skills_wire.rs:23`
  - `rupu-transcript/src/event.rs:804`
  - `skills.rs:44`
- Test: `crates/rupu-transcript/tests/it/skills_wire.rs`, `crates/rupu-agent/tests/it/agent_skills.rs`, `crates/rupu-runtime/tests/it/assembly_skills.rs`, `web/src/lib/transcriptLines.test.ts`

**Interfaces:**
- Consumes: `LayerDirs::with_overlay`, `OverlaySkill`, `Skill.shipped` (Task 3).
- Produces:
  ```rust
  // rupu-agent loader.rs
  pub fn load_agent_in(global: &Path, project: Option<&Path>, name: &str, overlay: &[rupu_skills::OverlaySkill]) -> Result<AgentSpec, AgentLoadError>;
  // load_agent(g, p, n) == load_agent_in(g, p, n, &[])
  // rupu-runtime spec.rs
  pub struct Overrides { /* existing */, pub skill_overlay: Vec<rupu_skills::OverlaySkill> }
  // rupu-runtime assembly/mod.rs
  pub async fn resolve_skill_requests(
      dirs: &rupu_skills::LayerDirs,
      label: &str, // the fd-retry label (agent name)
      requests: Vec<(rupu_skills::SkillReason, Vec<String>, Vec<String>)>,
  ) -> Result<Option<Arc<rupu_skills::ResolvedSkills>>, AssembleError>;
  // rupu-transcript
  pub struct SkillIndexEntry { /* existing */, #[serde(default, skip_serializing_if = "std::ops::Not::not")] pub shipped: bool }
  ```

- [ ] **Step 1: Write failing tests.**
  - **`skills_wire.rs`:**
    - An entry with `shipped: true` round-trips through JSON.
    - An old line with no `shipped` key reads as `false`.
    - `shipped: false` writes no `shipped` key.
    - `skill_index_line` renders `pdf (shipped)`, `pdf (preloaded, shipped)` and `pdf (not indexed, shipped)`.
  - **`agent_skills.rs`:** `load_agent_in` accepts an agent whose `skills: [pdf]` exists only in an overlay (write the agent file to `<home>/agents/a.md` and the skill to a separate tempdir). `load_agent` without the overlay fails with `AgentLoadError::Skill`.
  - **`assembly_skills.rs`:**
    - With `Overrides { skill_overlay: vec![<overlay pdf>], ..Default::default() }` and an agent granting `pdf`, `RunAssembler::resolve_skills` returns a `GrantedSkill` whose `skill.shipped` is true and whose `skill.dir` is the overlay dir.
    - A full mock-provider run records `skill_index` with `shipped: true`. Copy the nearest existing `assembly_skills.rs` run test and add the overlay.
  - **`transcriptLines.test.ts`:** the three suffix cases above.

- [ ] **Step 2: Run them and confirm they fail.**
  ```bash
  cargo test -p rupu-transcript --test it skills_wire::
  cargo test -p rupu-agent --test it agent_skills::
  cargo test -p rupu-runtime --test it assembly_skills::
  ```
  Then run `npx vitest run src/lib/transcriptLines.test.ts` in `crates/rupu-cp/web`.

- [ ] **Step 3: Implement.**
  - **`load_agent_in`:** `load_agent` builds `LayerDirs::for_layers(global, project)` at :149. Add `.with_overlay(overlay.to_vec())` there in a new `load_agent_in`, and make `load_agent` call it with `&[]`.
  - **`resolve_skill_requests`:** move the body of the private `resolve_run_skills` call and the fd retry out of `resolve_agent_skills` into it. `resolve_agent_skills` then builds its two requests and calls `resolve_skill_requests(dirs, &agent.name, requests)`. An empty request set returns `Ok(None)`.
  - **`RunAssembler::resolve_skills`:** build `for_layers(..).with_overlay(overrides.skill_overlay.clone())`.
  - **`write_skill_index`:** set `shipped: g.skill.shipped`.
  - **`skill_index_line`:** collect the qualifiers `preloaded` / `not indexed` / `shipped` and render `name (a, b)` when any exist.
  - **TS:** mirror the renderer, and add `shipped?: boolean` to the `skill_index` type.

- [ ] **Step 4: Run the tests.** Rerun the Step 2 commands, plus `cargo test -p rupu-cli --lib` (which compiles the literals), and vitest for `transcriptLines` and `transcriptView`. Expected: PASS.

- [ ] **Step 5: Clippy and commit.** Run clippy on rupu-agent, rupu-runtime, rupu-transcript and rupu-cli. `git commit -m "feat(skills): a run resolves through a shipped-skill overlay and records it"`

---

### Task 5: `rupu run --skill-bundle`, argv, and `--continue`

**Files:**
- Modify: `crates/rupu-runtime/src/argv.rs` (`AgentRun.skill_bundle: Option<String>`; `to_args` renders `--skill-bundle=<json>` as one element, right after the `--preload-skill`s)
- Modify: `crates/rupu-cli/src/cmd/run.rs`:
  - clap: `#[arg(long = "skill-bundle", hide = true, value_name = "JSON")] pub skill_bundle: Option<String>`;
  - `parse_launch_args` maps it back;
  - the run path near :663 / :740 / :944.
- Test: `crates/rupu-cli/tests/it/argv_roundtrip.rs`, `crates/rupu-runtime/src/argv.rs` unit tests, `crates/rupu-cli/tests/serial/` (new module `skill_bundle_run.rs`, added to `tests/serial/main.rs`)

**Interfaces:**
- Consumes: `rupu_skills::ship::{SkillBundleManifest, cache_dir, overlay_for}` (Task 2), `load_agent_in` and `Overrides.skill_overlay` (Task 4).
- Produces: the host-side contract that Tasks 6 and 7 rely on. `rupu run <agent> --skill-bundle='<manifest json>' …` runs with the overlay. The run writes `<global>/runs/<run_id>/skill_bundle.json`. `rupu run --continue <id>`, given no `--skill-bundle`, reuses the prior run's `skill_bundle.json` when present.

- [ ] **Step 1: Write failing tests.**
  - **`argv_roundtrip.rs`:** extend `agent_runs()` with a new mask bit (`0u32..(1 << 9)`). Bit 8 sets `r.skill_bundle = Some(r#"{"version":1,"skills":[{"layer":"global","name":"pdf","content_hash":"<64 a's>"}]}"#.into())`. Make `back()` carry `skill_bundle`.
  - **The `argv.rs` unit test:** `to_args` holds exactly one element starting with `--skill-bundle=` when it is set, and none when it is unset.
  - **`tests/serial/skill_bundle_run.rs`:**
    1. Build a `RUPU_HOME` with `agents/a.md` granting `skills: [pdf]` and NO `skills/` dir.
    2. Pack a `pdf` skill from a separate tempdir with `rupu_skills::ship::pack_skill`, and `store` it into `cache_dir(home)`.
    3. Run `rupu run a --skill-bundle=<manifest json> --prompt=hi` under the mock provider (copy the setup of an existing serial run test that uses `RUPU_MOCK_PROVIDER_SCRIPT` with `ENV_LOCK`, and `< /dev/null`).
    4. Assert exit 0, a transcript `skill_index` entry for `pdf` with `"shipped": true`, and `runs/<id>/skill_bundle.json` equal to the manifest.
    5. Second case: the same manifest with an unstored hash fails with a message containing `global:pdf` and `not in this host's skill cache`.
    6. Third case: `rupu run --continue <id>` after the first run, with no `--skill-bundle`, reuses the overlay. Its transcript also has `shipped: true`.
    7. Fourth case: a malformed `--skill-bundle` (bad JSON, or version 2) fails with an error naming `--skill-bundle`.

- [ ] **Step 2: Run them and confirm they fail.**
  ```bash
  cargo test -p rupu-runtime --lib argv
  cargo test -p rupu-cli --test it argv_roundtrip::
  cargo test -p rupu-cli --test serial skill_bundle_run:: < /dev/null
  ```

- [ ] **Step 3: Implement**, in `cmd/run.rs` before the agent load:
  1. Parse `args.skill_bundle` with `serde_json::from_str::<SkillBundleManifest>` and `validate()`. Wrap any error as `--skill-bundle: <e>`.
  2. On `--continue` with no flag, read `<global>/runs/<prev_id>/skill_bundle.json` if it exists. That is the same directory the prior run's `usage.jsonl` lives in; find how `--continue` locates the prior run and use its run id.
  3. `let overlay = overlay_for(&cache_dir(&global), &manifest)?` (an empty Vec when there's no manifest).
  4. Call `load_agent_in(&global, project, &args.agent, &overlay)` in place of `load_agent`.
  5. Set `Overrides.skill_overlay = overlay`.
  6. After the run id is known, write the manifest JSON to `<global>/runs/<run_id>/skill_bundle.json`, creating the directory.

  `AgentRun` gets `skill_bundle` in `new()` (None), in `to_args`, and in `parse_launch_args`.

- [ ] **Step 4: Run the tests.** Rerun the Step 2 commands. Expected: PASS.

- [ ] **Step 5: Clippy and commit.** `git commit -m "feat(cli): rupu run --skill-bundle runs with shipped skills"`

---

### Task 6: The receiving side: `rupu __skills` and the HTTP endpoints

**Files:**
- Create: `crates/rupu-cli/src/cmd/skills_helper.rs`
- Modify: `crates/rupu-cli/src/lib.rs`: a `Cmd::Skills { action }` variant `#[command(name = "__skills", hide = true)]` next to `__workspace` (:249); its dispatch arm next to :497; its `ensure_supported` arm next to :608.
- Modify: `crates/rupu-cp/src/node/protocol.rs` (`CAP_AGENT_SKILL_BUNDLE`, in `host_features()` only)
- Modify: `crates/rupu-cp/src/api/skills.rs` (two routes)
- Modify: `crates/rupu-cp/src/api/agents.rs`: `AgentRunBody.skill_bundle: Option<SkillBundleManifest>`, validated → `AgentLaunchRequest.skill_bundle`.
- Modify: `crates/rupu-cp/src/agent_launcher.rs`: `AgentLaunchRequest.skill_bundle: Option<SkillBundleManifest>`; `agent_run()` sets `run.skill_bundle = Some(serde_json::to_string(m)?)`.
- Test: `crates/rupu-cli/tests/it/skills_helper.rs` (new module), `crates/rupu-cp/tests/it/skills_api.rs` (or the existing skills API test module), `crates/rupu-cp/src/api/agents.rs` unit tests

**Interfaces:**
- Consumes: `ship::{missing, store, decode_stream, cache_dir, ShippedSkillRef, SkillBundleManifest, MAX_BUNDLE_BYTES}` (Task 2).
- Produces:
  - `rupu __skills missing <hash>:<layer>:<name>...` prints one line per missing ref in the same `hash:layer:name` form; exit 0.
  - `rupu __skills receive` reads `encode_stream` bytes on stdin, `store`s each, and prints `ok <hash>` per stored skill; exit 1 with the error on any failure.
  - HTTP: `POST /api/skills/bundle/missing` takes `{"skills":[ShippedSkillRef]}` and returns `{"missing":[ShippedSkillRef]}`. `PUT /api/skills/bundle/:hash?layer=&name=` takes a tar body (`DefaultBodyLimit::max(MAX_SHIPPED_SKILL_BYTES as usize + 64 KiB)`), stores it and returns `{"stored": "<hash>"}`; a bad archive is a 400 and a hash mismatch is a 400 with its message. Both run their file work in `spawn_blocking` and use the CP's `global_dir` as `<global>`.
  - `pub const CAP_AGENT_SKILL_BUNDLE: &str = "agent.skill_bundle";` with a doc comment in the style of `CAP_AGENT_SKILLS`: "host feature entry: this executor caches shipped skill content (`rupu __skills`, `/api/skills/bundle/*`) and `rupu run` takes `--skill-bundle`, so a placed unit runs with the coordinator's skills."

- [ ] **Step 1: Write failing tests.**
  - **`skills_helper.rs`** (rupu-cli `tests/it`; isolate `RUPU_HOME` the way existing helper tests do, e.g. via `assert_cmd` with `.env("RUPU_HOME", home)`):
    1. `missing` with one unknown ref prints it.
    2. `receive` with an encoded stream of a packed skill prints `ok <hash>`.
    3. A second `missing` prints nothing.
    4. `receive` with a stream whose tar has the wrong hash exits 1 and caches nothing.
  - **`skills_api.rs`:**
    - `missing` then `PUT` then `missing` returns empty.
    - `PUT` with a tampered body is a 400.
    - `PUT` with `name=../x` is a 400.
    - `GET /api/host/info` `features` contains `agent.skill_bundle`.
    - The existing `GET /api/skills/:layer/:name` still serves (route coexistence).
  - **agents.rs:** `run_agent` forwards a valid `skill_bundle` into the captured `AgentLaunchRequest`. Copy `run_agent_forwards_the_skills` (agents.rs:1582). A manifest with a bad hash is a 400 before launching.

- [ ] **Step 2: Run them and confirm they fail.**

- [ ] **Step 3: Implement.** `skills_helper.rs` follows `workspace_helper.rs`: `#[derive(Subcommand)] pub enum SkillsHelperAction { Missing { refs: Vec<String> }, Receive }` and `pub async fn handle(action) -> ExitCode` with `crate::output::diag::fail` on error. Use `crate::paths::global_dir()` as the root, and read stdin with `BufReader` into `decode_stream(.., MAX_BUNDLE_BYTES)`.

- [ ] **Step 4: Run the tests.** Expected: PASS. Also run `cargo test -p rupu-cp --lib` (host_features list tests) and `cargo test -p rupu-cp --test it host_http::`.

- [ ] **Step 5: Clippy and commit.** `git commit -m "feat(skills): hosts receive shipped skills over rupu __skills and HTTP"`

---

### Task 7: Coordinator side: `ship_skills` on every connector, with SSH and HTTP for real

**Files:**
- Create: `crates/rupu-cp/src/host/skill_ship.rs`; modify `crates/rupu-cp/src/host/mod.rs` (`pub mod skill_ship;`)
- Modify: `crates/rupu-cp/src/host/connector.rs` (required trait method)
- Modify: `crates/rupu-cp/src/host/{local.rs, ssh.rs, http.rs, tunnel.rs, bucket/connector.rs}`
- Modify every other `impl HostConnector`, including test fakes: grep `impl HostConnector for` and `impl rupu_cp::host::connector::HostConnector for` across `crates/`, including `crates/rupu-cli/src/fleet_unit_dispatcher.rs` ~:966 and ~:1123.
- Test: `crates/rupu-cp/tests/it/host_http.rs`, the ssh.rs unit tests (scripted `RemoteExec`), `crates/rupu-cp/tests/it/skill_ship.rs` (new module)

**Interfaces:**
- Consumes: `ship::{pack_skill, PackedSkill, SkillBundleManifest, ShippedSkillRef, encode_stream, MAX_BUNDLE_BYTES, ShipError}`; `CAP_AGENT_SKILL_BUNDLE` (Task 6).
- Produces, in `rupu_cp::host::skill_ship`:
  ```rust
  /// What a unit needs shipped: its resolved skills, packed on demand.
  pub struct SkillBundle { skills: Vec<Arc<rupu_skills::Skill>> }
  impl SkillBundle {
      pub fn new(skills: Vec<Arc<rupu_skills::Skill>>) -> Self;
      pub fn is_empty(&self) -> bool;
      /// Packs every skill (process-wide cache keyed by (root, stamp)), enforcing MAX_BUNDLE_BYTES; blocking — call from spawn_blocking.
      pub fn pack(&self) -> Result<Vec<Arc<rupu_skills::ship::PackedSkill>>, rupu_skills::ship::ShipError>;
  }
  pub fn manifest_of(packed: &[Arc<PackedSkill>]) -> SkillBundleManifest;
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub enum ShipOutcome { Shipped(SkillBundleManifest), NotNeeded, Unsupported(String) }
  /// Per-connector memo: hashes this host confirmed, plus a ship lock (deviation 3).
  #[derive(Default)] pub struct ShipMemo { /* confirmed: Mutex<HashSet<String>>, lock: tokio::sync::Mutex<()> */ }
  impl ShipMemo {
      pub async fn ship<F, Fut>(&self, packed: &[Arc<PackedSkill>], transfer: F) -> Result<(), HostConnectorError>
      where F: FnOnce(Vec<Arc<PackedSkill>>) -> Fut, Fut: Future<Output = Result<(), HostConnectorError>>;
      // takes the lock; filters confirmed; if none left returns Ok without calling transfer; else transfer(unconfirmed) and on Ok marks them confirmed
  }
  // HostConnector (required, no default):
  async fn ship_skills(&self, bundle: &SkillBundle) -> Result<ShipOutcome, HostConnectorError>;
  ```
  - **Local:** `Ok(ShipOutcome::NotNeeded)`, and it never packs.
  - **Tunnel and bucket (Plan 1):** `Ok(ShipOutcome::Unsupported(format!("host {id}: shipping skills over this transport is not built yet")))`.
  - **Test fakes:** `Ok(ShipOutcome::NotNeeded)` unless the test scripts otherwise.
  - **SSH and HTTP:** as described in Step 3.

- [ ] **Step 1: Write failing tests.**
  - **`skill_ship.rs`:**
    - `ShipMemo::ship` called concurrently by 20 tasks with the same packed set invokes `transfer` exactly once (count with an `AtomicUsize`).
    - A later call with the same set invokes it zero times.
    - A failed transfer leaves the hashes unconfirmed, so the next call transfers again.
    - `SkillBundle::pack` over two skills totalling more than `MAX_BUNDLE_BYTES` (sparse files, as in Task 1) returns `BundleTooLarge`.
  - **ssh.rs unit tests**, using the scripted `RemoteExec` fakes already in the module (~:4499):
    - With the remote gate printing the unsupported marker: `ShipOutcome::Unsupported` naming the host.
    - With `missing` printing one ref: exactly one `run_bytes` call whose command contains `__skills receive` and whose stdin decodes (`decode_stream`) to that ref, and the outcome `Shipped(manifest)` listing every skill.
    - A second ship of the same bundle on the same connector makes no remote calls.
    - A receive reply missing an `ok <hash>` line is an error.
  - **`host_http.rs`** (httpmock peer):
    - The peer's `/api/host/info` lacks `agent.skill_bundle`: `Unsupported` and no POST.
    - It has the feature and `missing` returns one: one `PUT /api/skills/bundle/<hash>`, then `Shipped`.
    - A second ship: no calls.
    - A real in-process remote CP (`serve_cp`, as in the session-skills tests at `host_http.rs:995+`) stores a shipped skill, and a following `launch_agent` with `skill_bundle` posts `skill_bundle` in the body.

- [ ] **Step 2: Run them and confirm they fail.**
  ```bash
  cargo test -p rupu-cp --test it skill_ship::
  cargo test -p rupu-cp --test it host_http::
  cargo test -p rupu-cp --lib ssh
  ```

- [ ] **Step 3: Implement.**
  - **SSH:**
    1. Pack in `spawn_blocking`.
    2. Run the gate plus missing as one command: `skills_bundle_gate(<build_remote_command(["rupu","__skills","missing", refs...])>)`. That is `skills_gate`'s shape with `CAP_AGENT_SKILL_BUNDLE` and a new marker `rupu-skills: bundle-unsupported`.
    3. The marker maps to `Ok(ShipOutcome::Unsupported("host {id}: rupu there predates shipped skills (no agent.skill_bundle); upgrade rupu there. The unit uses that host's installed skills."))`.
    4. Run the receive step inside `ShipMemo::ship`: `self.exec.run_bytes(&build_remote_command(["rupu","__skills","receive"]), Some(encode_stream(..)))`. Require one `ok <hash>` line per sent skill.
    5. Return `Shipped(manifest_of(&packed))`. The manifest lists EVERY skill, including ones the host already had.
  - **HTTP:**
    1. `peer_info()` then `require_in(info, CAP_AGENT_SKILL_BUNDLE, "shipped skills", ..)`. An `Err(Unsupported(m))` becomes `Ok(ShipOutcome::Unsupported(m))`.
    2. Pack.
    3. `POST /api/skills/bundle/missing`.
    4. Inside `ShipMemo::ship`, `PUT` each missing one.
    5. In `launch_agent`, add `body["skill_bundle"] = serde_json::to_value(m)` when `req.skill_bundle` is `Some`.
  - **SSH `launch_agent`** renders `--skill-bundle` through `agent_run()` and `to_shell` with no extra code. Confirm this with a unit test asserting the launched command contains `--skill-bundle=`.

- [ ] **Step 4: Run the tests.** Rerun the Step 2 commands, plus `cargo test -p rupu-cli --lib fleet_unit_dispatcher` (fakes compile). Expected: PASS.

- [ ] **Step 5: Clippy and commit.** Run clippy on rupu-cp and rupu-cli. `git commit -m "feat(cp): connectors ship a unit's skills to SSH and HTTP hosts"`

---

### Task 8: The dispatcher: resolve, ship, launch, warn

**Files:**
- Modify: `crates/rupu-cli/src/fleet_unit_dispatcher.rs`:
  - `FleetUnitDispatcher.project: Option<PathBuf>`;
  - `new(registry, global, project)`, `from_connector(conn, global, project)`;
  - `build_dispatcher_if_needed(.., project_root: Option<&Path>)`;
  - `dispatch_unit`.
- Modify: the callers `crates/rupu-cli/src/cmd/workflow.rs:3897, 5674, 5907` and `resume.rs:482`. Pass the project root that the same call site gives its run's `AssemblyContext.project_root`.
- Modify: `crates/rupu-orchestrator/src/runner.rs`:
  - `UnitOutcome.warnings: Vec<String>` and `UnitFailure.warnings: Vec<String>` (default empty; update `From<RunError> for UnitFailure` and every constructor);
  - emit each as `Event::StepWarning` at the placed call site (~:8035) and the fan-out sites (~:9331, ~:9482), the way `ingest_remote_unit_coverage` (:7820) emits.
- Test: the `fleet_unit_dispatcher.rs` unit tests (fake connectors) and `crates/rupu-orchestrator/tests/it/` (a placed-step test that asserts the warning event; find the existing test that drives a fake `UnitDispatcher` for placed steps and extend it)

**Interfaces:**
- Consumes: `resolve_skill_requests` (Task 4), `rupu_agent::find_agent`, `SkillBundle` / `ShipOutcome` / `ship_skills` (Task 7), `AgentLaunchRequest.skill_bundle` (Task 6).
- Produces: placed units launched with `skill_bundle` when shipped, a downgrade warning when unsupported, and errors prefixed `host <id>: ` (deviation 4).

- [ ] **Step 1: Write failing tests**, in the dispatcher unit tests with the existing fake connectors, extended to record `ship_skills` calls and the `AgentLaunchRequest`:
  1. A unit whose agent (written under the test's global `agents/`) grants `pdf`, with step skills `[notes]`, both installed in the coordinator's global `skills/`. The fake answers `Shipped(m)`. Assert:
     - the fake saw a `SkillBundle` of `pdf` and `notes`;
     - the launch request has `skill_bundle == Some(m)`;
     - `warnings` is empty.
  2. The same unit with the fake answering `Unsupported("host h: …")`:
     - the launch request has `skill_bundle == None` and still carries `skills: [notes]`;
     - the outcome has one warning containing `host h`.
  3. The coordinator has NO agent file for the unit's agent, with step skills `[notes]`: only `notes` is in the bundle (Review Focus 5).
  4. A step skill not installed on the coordinator: `Err(UnitFailure)` whose message starts `host h: ` and names the skill. The fake saw no launch.
  5. A fake whose `launch_agent` fails with `Unreachable("boom")`: the error is `host h: …boom` and not double-prefixed.
  6. **Orchestrator:** a placed step whose dispatcher returns a `UnitOutcome` with `warnings: ["w"]` emits `Event::StepWarning { message: "w", .. }` on the run's event sink.

- [ ] **Step 2: Run them and confirm they fail.**
  ```bash
  cargo test -p rupu-cli --lib fleet_unit_dispatcher
  cargo test -p rupu-orchestrator --test it <placed-step module>::
  ```

- [ ] **Step 3: Implement `dispatch_unit`**, between `resolve(host)` and `launch_agent`:
  1. Build the requests:
     - `let dirs = LayerDirs::for_layers(&self.global, self.project.as_ref().map(|p| p.join(".rupu")).as_deref());`
     - `let agent = rupu_agent::find_agent(&self.global, self.project.as_ref().map(|p| p.join(".rupu")).as_deref(), &unit.agent).ok();` (`find_agent` takes the same `project` argument `load_agent` does; check whether it is the project root or its `.rupu` and pass the matching one);
     - `(Agent, agent.skills, agent.preload_skills)` when `agent` is `Some`, plus `(Launch, unit.skills, unit.preload_skills)`.
  2. `resolve_skill_requests(&dirs, &unit.agent, requests).await`. An error is `Err(UnitFailure)` with `NotLaunched` coverage.
  3. When there are any skills: `let bundle = SkillBundle::new(resolved.entries().iter().map(|g| g.skill.clone()).collect());` then `conn.ship_skills(&bundle).await`:
     - `Shipped(m)` → `skill_bundle = Some(m)`;
     - `NotNeeded` → `None`;
     - `Unsupported(msg)` → `None` and push `msg` onto `warnings`;
     - `Err(e)` → a failure with `NotLaunched`.
  4. Pass `skill_bundle` in the `AgentLaunchRequest` and carry `warnings` into the returned `UnitOutcome` or `UnitFailure`.
  5. Wrap every error string produced after `resolve(host)` with `prefix_host(host, msg)`, which adds `host <id>: ` unless the message already starts with it. Apply the same wrap to `UnitOutcome.error` when the remote run failed.

- [ ] **Step 4: Run the tests.** Rerun the Step 2 commands, plus `cargo test -p rupu-cli --test it` and `cargo test -p rupu-orchestrator --test it`. Expected: PASS. Fix any existing assertion on a placed unit's error text so that it expects the `host <id>: ` prefix.

- [ ] **Step 5: Clippy and commit.** `git commit -m "feat(fleet): placed units ship their skills, with a downgrade warning on older hosts"`

---

### Task 9: Docs

**Files:** `docs/skills.md` (the remote / placed-units section), `docs/cp-api.md` (`/api/skills/bundle/*`, `skill_bundle` on agent run, the `agent.skill_bundle` feature), the Agent Skills spec §9 (a pointer to the new spec), and `CLAUDE.md`:
- the `rupu-skills` entry gets `ship`: pack/unpack, cache, overlay;
- the `rupu-cp` entry gets `ship_skills`, `ShipMemo` and the capability;
- add the Read-first line for this spec and plan.

- [ ] **Step 1:** Update each file to match what Tasks 1–8 built. Note deviations 1–4 where the docs describe the behaviour.
- [ ] **Step 2:** `git commit -m "docs(skills): shipped skills on placed units"`

---

## Verification before the PR

Run these:
```bash
cargo test -p rupu-skills
cargo test -p rupu-transcript
cargo test -p rupu-agent --lib
cargo test -p rupu-agent --test it
cargo test -p rupu-runtime --test it assembly_skills::
cargo test -p rupu-orchestrator --test it
cargo test -p rupu-cp --lib
cargo test -p rupu-cp --test it
cargo test -p rupu-cli --lib
cargo test -p rupu-cli --test it
cargo test -p rupu-cli --test serial skill_bundle_run:: < /dev/null
cargo clippy -p rupu-skills -p rupu-agent -p rupu-runtime -p rupu-transcript -p rupu-orchestrator -p rupu-cp -p rupu-cli --all-targets
```
Then, in `crates/rupu-cp/web`: `npx vitest run` and `npm run build`.

Then an end-to-end check on a real host is optional, and only with matt's go-ahead: SSH to a scratch host (never burst connections at matt's hosts), place a unit with a skill the host lacks, and confirm it runs with `(shipped)` in the transcript.

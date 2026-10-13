# Fleet skills Plan 1: pinned skill bundles, `Section9Labs/rupu-skills` v1.0.0, network fleet

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `rupu fleet install` installs the stock fleet's skills from `Section9Labs/rupu-skills` at a pinned commit, and the network fleet uses real, scope-enforcing recon, enumeration, work-unit and report skills.

**Architecture:** Three stages, in order. (A) rupu machinery: a `skills` list on the profile `[bundle]`, a `rupu_skills::pinned` module that installs pins through the existing install and record code, `--skills-from` as a second `Fetch`, lockstep tests, a networked pin check in CI. (B) the new public `rupu-skills` repository: a shared Python scope gate and preflight, five skills (four authored with Claude Mythos and reviewed, one vendored), CI, lab verification, tag `v1.0.0`. (C) a rupu PR that pins `v1.0.0`, grants the skills to the fleet agents and fans the network workflow out over work units.

**Tech Stack:** Rust (rupu workspace: `rupu-coverage`, `rupu-skills`, `rupu-cli`; serde, git2, tempfile), Python 3 standard library (skill scripts, `unittest`), GitHub Actions, nmap / masscan / naabu on Kali.

**Spec:** `docs/superpowers/specs/2026-10-13-rupu-fleet-skills-design.md`

## Global Constraints

- Both repositories are public. Nothing from private assessment material (matt's private assessment notes or his private agents in `~/.rupu/agents`) goes into skills, fixtures, tests or docs, even renamed. Fixtures use documentation address ranges (192.0.2.0/24, 198.51.100.0/24, 203.0.113.0/24) and `example.com` / `example.net` names.
- Pins are full 40-character lowercase hex commits; `tag` is display only and never fetched.
- Skill frontmatter keys: only `name`, `description`, `license`, `compatibility`, `metadata` (string values). `name` equals the directory name.
- rupu-skills versioning is its own semver (`v1.0.0`), independent of rupu.
- Skill scripts: Python 3 standard library only; every script that touches a target calls the scope gate first.
- Default scan rate 1000 pps; hard ceiling 10000 pps; above the ceiling needs `--allow-high-rate` AND a `max-rate` line in the scope file.
- `cargo test` stays offline. Integration tests live in `crates/<c>/tests/it/` (one binary per crate).
- rupu work goes through a feature branch and a PR; push with an explicit refspec (`git push origin <branch>:<branch>`); never bare `git stash` / `git stash pop`.
- Run only the touched test modules (`cargo test -p <crate> --test it <module>::`), never a cold full-workspace test.
- Never run package-wide `cargo fmt`; format only touched files (`rustfmt --edition 2021 <file>`).
- Creating `Section9Labs/rupu-skills` on GitHub needs matt's explicit go-ahead in chat (Task B11).
- Lab scanning is authorized only against kali-b (192.168.64.8) from kali6 (192.168.64.6). SSH key `~/.ssh/id_ed25519_kali`. Batch commands into one `ssh` invocation; never burst connections.

## Review Focus

1. **A dirty `--skills-from` checkout.** A checkout with `__pycache__/` or any untracked or ignored file under `skills/` (left by running the script tests) must be refused, because `install` copies every file in the skill directory; the refusal must say to run `git clean -fdx skills/`. Pinned in Task A3.
2. **A hostname target that resolves to a private address.** `app.example.com` is in scope by name but DNS points it at 10.0.0.5; with no network entry allowing 10.0.0.5, the gate must refuse. Pinned in Task B1.
3. **A skill installed by hand under a fleet skill's name.** `rupu skill install ./my-network-recon` then `rupu fleet install`: the user's skill is kept, never overwritten without `--force`. Pinned in Task A2.
4. **One pin's fetch fails while another source succeeds.** The good skills install; the command exits 1 and lists only the missing ones. Pinned in Task A2.
5. **A SKILL.md outside `skills/` in rupu-skills** (for example a test fixture) makes `find_skills` report a duplicate name and breaks every install. Pinned by a CI check in Task B3.

---

## Stage A — rupu machinery

Branch: `feat/fleet-skill-pins` from a fresh `origin/main` (`git fetch origin main:refs/remotes/origin/main` first).

### Task A1: `BundleSkill` in the profile `[bundle]`

**Files:**
- Modify: `crates/rupu-coverage/src/profile/package.rs` (the `Bundle` struct and its `tests` module)
- Modify: `crates/rupu-coverage/src/lib.rs` (re-export `BundleSkill` next to `Bundle`, if `Bundle` is re-exported there)

**Interfaces:**
- Produces: `rupu_coverage::profile::BundleSkill { name: String, source: String, tag: String, commit: String }` and `Bundle.skills: Vec<BundleSkill>` (`#[serde(default)]`). Parsing rejects a `commit` that is not exactly 40 lowercase hex characters.

- [ ] **Step 1: Write the failing tests** in `package.rs`'s `mod tests`:

```rust
    const PIN: &str = "0123456789abcdef0123456789abcdef01234567";

    #[test]
    fn bundle_skills_parse_with_a_full_commit() {
        let p = parse_profile(&format!(
            r#"
id = "x"
name = "x"

[bundle]
agents = ["recon"]
skills = [
  {{ name = "network-recon", source = "Section9Labs/rupu-skills", tag = "v1.0.0", commit = "{PIN}" }},
]
"#
        ))
        .unwrap();
        assert_eq!(p.bundle.skills.len(), 1);
        let s = &p.bundle.skills[0];
        assert_eq!(s.name, "network-recon");
        assert_eq!(s.source, "Section9Labs/rupu-skills");
        assert_eq!(s.tag, "v1.0.0");
        assert_eq!(s.commit, PIN);
    }

    #[test]
    fn bundle_skills_default_to_empty() {
        let p = parse_profile("id = \"x\"\nname = \"x\"\n[bundle]\nagents = [\"a\"]\n").unwrap();
        assert!(p.bundle.skills.is_empty());
    }

    #[test]
    fn a_bundle_skill_commit_must_be_a_full_lowercase_sha() {
        for bad in ["v1.0.0", "main", "0123456", "0123456789ABCDEF0123456789ABCDEF01234567", "0123456789abcdef0123456789abcdef0123456z"] {
            let err = parse_profile(&format!(
                "id = \"x\"\nname = \"x\"\n[bundle]\nskills = [{{ name = \"a\", source = \"o/r\", tag = \"t\", commit = \"{bad}\" }}]\n"
            ));
            let msg = format!("{}", err.expect_err(bad));
            assert!(msg.contains("40-character"), "{bad}: {msg}");
        }
    }

    #[test]
    fn a_bundle_skill_rejects_unknown_keys() {
        let err = parse_profile(&format!(
            "id = \"x\"\nname = \"x\"\n[bundle]\nskills = [{{ name = \"a\", source = \"o/r\", tag = \"t\", commit = \"{PIN}\", ref = \"main\" }}]\n"
        ));
        assert!(err.is_err(), "deny_unknown_fields must reject `ref`");
    }
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p rupu-coverage --lib profile::package::tests::`
Expected: compile error, no field `skills` on `Bundle`.

- [ ] **Step 3: Implement.** In `package.rs`, extend `Bundle` and add `BundleSkill`:

```rust
/// Launcher prefill only (never a tool or skill grant): the agents, tools,
/// workflows and skills an engagement typically uses.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bundle {
    #[serde(default)]
    pub agents: Vec<String>,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub workflows: Vec<String>,
    /// The skills `rupu fleet install` installs for this profile's agents,
    /// each pinned to a commit.
    #[serde(default)]
    pub skills: Vec<BundleSkill>,
}

/// One bundled skill: what `rupu skill install` would be given, pinned.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BundleSkill {
    /// The skill's directory name in the source.
    pub name: String,
    /// `owner/repo` or an https git URL.
    pub source: String,
    /// The release the commit belongs to. Shown, never fetched.
    pub tag: String,
    /// What is fetched: a full commit, so a moved tag changes nothing.
    #[serde(deserialize_with = "full_commit")]
    pub commit: String,
}

fn full_commit<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    let s = String::deserialize(d)?;
    if s.len() == 40 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        Ok(s)
    } else {
        Err(serde::de::Error::custom(format!(
            "commit {s:?} must be a full 40-character lowercase hex SHA; a tag or branch can move"
        )))
    }
}
```

Re-export `BundleSkill` wherever `Bundle` is re-exported (`grep -n "Bundle" crates/rupu-coverage/src/lib.rs crates/rupu-coverage/src/profile/mod.rs`).

- [ ] **Step 4: Run the tests**

Run: `cargo test -p rupu-coverage --lib profile::`
Expected: PASS (the new four and the existing profile tests, including every built-in profile parsing).

- [ ] **Step 5: Docs.** In `docs/engagement-profiles.md`, the `[bundle]` paragraph near line 464 ("`[bundle]` holds `agents`, `workflows` and `tools`…"): add `skills`, an array of `{ name, source, tag, commit }` tables; `commit` is a full SHA and is what is fetched; `tag` is shown only; like the rest of the bundle, a bundled skill grants nothing — `rupu fleet install` installs the built-in bundles' skills. Mirror one sentence in the `[bundle]` mention in `docs/using-rupu.md`.

- [ ] **Step 6: Commit**

```bash
rustfmt --edition 2021 crates/rupu-coverage/src/profile/package.rs
git add crates/rupu-coverage docs/engagement-profiles.md docs/using-rupu.md
git commit -m "feat(coverage): pinned skills in a profile's [bundle]"
```

### Task A2: `rupu_skills::pinned` — install pins through the normal install path

**Files:**
- Create: `crates/rupu-skills/src/pinned.rs`
- Modify: `crates/rupu-skills/src/lib.rs` (`pub mod pinned;` + re-exports)
- Modify: `crates/rupu-skills/src/install.rs` (extract the record-path join so `install_from` and `pinned` share it)
- Create: `crates/rupu-skills/tests/it/pinned.rs`; Modify: `crates/rupu-skills/tests/it/main.rs` (`mod pinned;`)

**Interfaces:**
- Consumes: `parse_source`, `Source`, `GitSource`, `Fetch`, `FetchedTree` (`source.rs`); `find_skills`, `install_selected`, `Selection`, `Tracking`, `Installed`, `recover_replaced` (`install.rs`); `read_record`, `write_record`, `SourceRecord`, `RECORD_VERSION`, `now_secs` (`record.rs`); `load_skill`, `Layer` (`skill.rs`).
- Produces:
  - `pub struct Pin { pub name: String, pub source: String, pub tag: String, pub commit: String }`
  - `pub enum PinOutcome { Created, Skipped, Updated { from: Option<String> }, Replaced, Kept(KeptReason), Failed(String) }`
  - `pub enum KeptReason { LocallyModified, OtherSource }`
  - `pub struct PinReport { pub results: Vec<(Pin, PinOutcome)> }` with `pub fn missing(&self) -> Vec<&Pin>` (the `Failed` ones)
  - `pub fn install_pins(pins: &[Pin], skills_dir: &Path, layer: Layer, force: bool, fetcher: &dyn Fetch) -> PinReport`
  - in `install.rs`: `pub(crate) fn record_path_of(base: Option<&str>, rel: &str) -> Option<String>`

Behavior per pin (decided before any fetch, from what is on disk):

| On disk | `force` false | `force` true |
|---|---|---|
| no `<skills_dir>/<name>/SKILL.md` | install → `Created` | same |
| record `url` == the pin's url, content hash == record hash, record `commit` == pin | `Skipped` | `Skipped` |
| record `url` == the pin's url, content hash == record hash, other commit | install → `Updated { from: record.commit }` | same |
| record `url` == the pin's url, content hash ≠ record hash | `Kept(LocallyModified)` | install → `Replaced` |
| no record, or record `url` ≠ the pin's url | `Kept(OtherSource)` | install → `Replaced` |
| record unreadable, or `load_skill` fails | `Failed(<reason>)` | install → `Replaced` |

Pins to install are grouped by (url, commit); each group is fetched once with `GitSource { url, git_ref: Some(commit), path: None }`. If the fetched `commit` is not `Some(pin.commit)`, every pin in the group is `Failed("fetched <got>, the fleet pins <want>")`. Each pin installs on its own (`Selection::Named(vec![name])`, `force: true` because the table already decided), so one bad skill never stops another. The record written is the one `rupu skill install` writes for a git source: `source` = the pin's `source` string, `url`, `ref` = the commit, `path` = where the skill sits in the repository, `commit` = the fetched commit, `content_hash`, `installed_at`.

- [ ] **Step 1: Write the failing tests** in `crates/rupu-skills/tests/it/pinned.rs`:

```rust
use rupu_skills::pinned::{install_pins, KeptReason, Pin, PinOutcome};
use rupu_skills::record::read_record;
use rupu_skills::{Fetch, FetchedTree, GitSource, Layer, SourceError};
use std::cell::RefCell;
use std::path::{Path, PathBuf};

use crate::support::write_skill;

const URL: &str = "https://github.com/Section9Labs/rupu-skills";
const C1: &str = "1111111111111111111111111111111111111111";
const C2: &str = "2222222222222222222222222222222222222222";

/// Serves a generated repository (`skills/<name>/SKILL.md` per name) and
/// reports whatever commit it is told to; records every fetch.
struct FixtureFetch {
    root: PathBuf,
    commit: String,
    fail_url: Option<String>,
    fetches: RefCell<Vec<(String, Option<String>)>>,
}

impl FixtureFetch {
    fn new(root: &Path, names: &[&str], body: &str, commit: &str) -> Self {
        for n in names {
            write_skill(&root.join("skills"), n, n, &format!("{n} {body}"));
        }
        Self {
            root: root.to_owned(),
            commit: commit.to_owned(),
            fail_url: None,
            fetches: RefCell::new(Vec::new()),
        }
    }
}

impl Fetch for FixtureFetch {
    fn fetch(&self, src: &GitSource) -> Result<FetchedTree, SourceError> {
        self.fetches.borrow_mut().push((src.url.clone(), src.git_ref.clone()));
        if self.fail_url.as_deref() == Some(src.url.as_str()) {
            return Err(SourceError::Clone("network is down".into()));
        }
        Ok(FetchedTree::local_with_commit(self.root.clone(), Some(self.commit.clone())))
    }
}

fn pin(name: &str, commit: &str) -> Pin {
    Pin {
        name: name.into(),
        source: "Section9Labs/rupu-skills".into(),
        tag: "v1.0.0".into(),
        commit: commit.into(),
    }
}

fn outcomes(r: &rupu_skills::pinned::PinReport) -> Vec<(String, PinOutcome)> {
    r.results.iter().map(|(p, o)| (p.name.clone(), o.clone())).collect()
}

#[test]
fn a_first_install_creates_every_skill_with_a_pinned_record() {
    let src = tempfile::tempdir().unwrap();
    let dest = tempfile::tempdir().unwrap();
    let f = FixtureFetch::new(src.path(), &["network-recon", "work-units"], "v1", C1);
    let r = install_pins(&[pin("network-recon", C1), pin("work-units", C1)], dest.path(), Layer::Global, false, &f);
    assert_eq!(
        outcomes(&r),
        vec![("network-recon".into(), PinOutcome::Created), ("work-units".into(), PinOutcome::Created)]
    );
    assert_eq!(f.fetches.borrow().len(), 1, "one fetch per (url, commit)");
    assert_eq!(f.fetches.borrow()[0], (URL.to_owned(), Some(C1.to_owned())));
    let rec = read_record(dest.path(), "network-recon").unwrap().unwrap();
    assert_eq!(rec.source, "Section9Labs/rupu-skills");
    assert_eq!(rec.url.as_deref(), Some(URL));
    assert_eq!(rec.git_ref.as_deref(), Some(C1));
    assert_eq!(rec.commit.as_deref(), Some(C1));
    assert_eq!(rec.path.as_deref(), Some("skills/network-recon"));
    assert!(r.missing().is_empty());
}

#[test]
fn a_second_install_at_the_same_pin_skips_without_fetching() {
    let src = tempfile::tempdir().unwrap();
    let dest = tempfile::tempdir().unwrap();
    let f = FixtureFetch::new(src.path(), &["work-units"], "v1", C1);
    install_pins(&[pin("work-units", C1)], dest.path(), Layer::Global, false, &f);
    let r = install_pins(&[pin("work-units", C1)], dest.path(), Layer::Global, false, &f);
    assert_eq!(outcomes(&r), vec![("work-units".into(), PinOutcome::Skipped)]);
    assert_eq!(f.fetches.borrow().len(), 1);
}

#[test]
fn a_new_pin_updates_an_unmodified_skill() {
    let src1 = tempfile::tempdir().unwrap();
    let src2 = tempfile::tempdir().unwrap();
    let dest = tempfile::tempdir().unwrap();
    install_pins(&[pin("work-units", C1)], dest.path(), Layer::Global, false, &FixtureFetch::new(src1.path(), &["work-units"], "v1", C1));
    let r = install_pins(&[pin("work-units", C2)], dest.path(), Layer::Global, false, &FixtureFetch::new(src2.path(), &["work-units"], "v2", C2));
    assert_eq!(outcomes(&r), vec![("work-units".into(), PinOutcome::Updated { from: Some(C1.into()) })]);
    let md = std::fs::read_to_string(dest.path().join("work-units/SKILL.md")).unwrap();
    assert!(md.contains("work-units v2"));
    assert_eq!(read_record(dest.path(), "work-units").unwrap().unwrap().commit.as_deref(), Some(C2));
}

#[test]
fn a_locally_modified_skill_is_kept_and_force_replaces_it() {
    let src1 = tempfile::tempdir().unwrap();
    let src2 = tempfile::tempdir().unwrap();
    let dest = tempfile::tempdir().unwrap();
    install_pins(&[pin("work-units", C1)], dest.path(), Layer::Global, false, &FixtureFetch::new(src1.path(), &["work-units"], "v1", C1));
    std::fs::write(dest.path().join("work-units/scripts/run.sh"), "#!/bin/sh\necho mine\n").unwrap();
    let f2 = FixtureFetch::new(src2.path(), &["work-units"], "v2", C2);
    let kept = install_pins(&[pin("work-units", C2)], dest.path(), Layer::Global, false, &f2);
    assert_eq!(outcomes(&kept), vec![("work-units".into(), PinOutcome::Kept(KeptReason::LocallyModified))]);
    assert_eq!(std::fs::read_to_string(dest.path().join("work-units/scripts/run.sh")).unwrap(), "#!/bin/sh\necho mine\n");
    let forced = install_pins(&[pin("work-units", C2)], dest.path(), Layer::Global, true, &f2);
    assert_eq!(outcomes(&forced), vec![("work-units".into(), PinOutcome::Replaced)]);
}

#[test]
fn a_hand_installed_skill_of_the_same_name_is_kept() {
    let src = tempfile::tempdir().unwrap();
    let dest = tempfile::tempdir().unwrap();
    write_skill(dest.path(), "network-recon", "network-recon", "my own");
    let f = FixtureFetch::new(src.path(), &["network-recon"], "v1", C1);
    let r = install_pins(&[pin("network-recon", C1)], dest.path(), Layer::Global, false, &f);
    assert_eq!(outcomes(&r), vec![("network-recon".into(), PinOutcome::Kept(KeptReason::OtherSource))]);
    assert!(f.fetches.borrow().is_empty(), "nothing to install, nothing fetched");
    let md = std::fs::read_to_string(dest.path().join("network-recon/SKILL.md")).unwrap();
    assert!(md.contains("my own"));
}

#[test]
fn a_fetched_commit_other_than_the_pin_installs_nothing() {
    let src = tempfile::tempdir().unwrap();
    let dest = tempfile::tempdir().unwrap();
    let f = FixtureFetch::new(src.path(), &["work-units"], "v1", C2);
    let r = install_pins(&[pin("work-units", C1)], dest.path(), Layer::Global, false, &f);
    match &r.results[0].1 {
        PinOutcome::Failed(msg) => assert!(msg.contains(C1) && msg.contains(C2), "{msg}"),
        other => panic!("expected Failed, got {other:?}"),
    }
    assert!(!dest.path().join("work-units").exists());
    assert_eq!(r.missing().len(), 1);
}

#[test]
fn one_failing_source_does_not_stop_another() {
    let src = tempfile::tempdir().unwrap();
    let dest = tempfile::tempdir().unwrap();
    let mut f = FixtureFetch::new(src.path(), &["work-units", "other"], "v1", C1);
    f.fail_url = Some("https://github.com/someone/elsewhere".into());
    let mut other = pin("other", C1);
    other.source = "someone/elsewhere".into();
    let r = install_pins(&[pin("work-units", C1), other], dest.path(), Layer::Global, false, &f);
    assert_eq!(r.results[0].1, PinOutcome::Created);
    assert!(matches!(r.results[1].1, PinOutcome::Failed(ref m) if m.contains("network is down")));
    assert_eq!(r.missing().iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["other"]);
}

#[test]
fn a_pin_missing_from_the_fetched_tree_fails_alone() {
    let src = tempfile::tempdir().unwrap();
    let dest = tempfile::tempdir().unwrap();
    let f = FixtureFetch::new(src.path(), &["work-units"], "v1", C1);
    let r = install_pins(&[pin("work-units", C1), pin("no-such-skill", C1)], dest.path(), Layer::Global, false, &f);
    assert_eq!(r.results[0].1, PinOutcome::Created);
    assert!(matches!(r.results[1].1, PinOutcome::Failed(_)));
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p rupu-skills --test it pinned::`
Expected: compile error, unresolved `rupu_skills::pinned`.

- [ ] **Step 3: Extract the record-path join in `install.rs`.** In `install_from`'s `Source::Git(g)` arm, the `joined` match becomes a call to a new function, used by `pinned` too:

```rust
/// Where an installed skill sits in its git source, for its record: the
/// source's `#path` joined with the skill's place under it.
pub(crate) fn record_path_of(base: Option<&str>, rel: &str) -> Option<String> {
    match (base, rel) {
        (Some(p), "") => Some(p.to_owned()),
        (Some(p), r) => Some(format!("{p}/{r}")),
        (None, "") => None,
        (None, r) => Some(r.to_owned()),
    }
}
```

and in `install_from`: `Source::Git(g) => (Some(g.url.clone()), g.git_ref.clone(), record_path_of(g.path.as_deref(), &inst.rel), None),`.

Run: `cargo test -p rupu-skills --test it install:: update::` — Expected: PASS (unchanged behavior).

- [ ] **Step 4: Write `crates/rupu-skills/src/pinned.rs`:**

```rust
//! Installing skills pinned to a commit: the stock fleet's skills, which
//! `rupu fleet install` takes from the built-in profiles' `[bundle]` (spec
//! `docs/superpowers/specs/2026-10-13-rupu-fleet-skills-design.md` §5.2).
//! A pin installs through the same path and writes the same source record
//! as `rupu skill install <source> --ref <commit>`, so `rupu skill update`
//! knows where a fleet skill came from.

use crate::install::{find_skills, install_selected, record_path_of, Installed, Selection, Tracking};
use crate::record::{now_secs, read_record, write_record, SourceRecord, RECORD_VERSION};
use crate::skill::{load_skill, Layer};
use crate::source::{parse_source, Fetch, GitSource, Source};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pin {
    pub name: String,
    pub source: String,
    pub tag: String,
    pub commit: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeptReason {
    /// The installed copy no longer matches its record: edited locally.
    LocallyModified,
    /// No record, or a record naming another source: installed by hand.
    OtherSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PinOutcome {
    Created,
    Skipped,
    Updated { from: Option<String> },
    Replaced,
    Kept(KeptReason),
    Failed(String),
}

#[derive(Debug, Default)]
pub struct PinReport {
    pub results: Vec<(Pin, PinOutcome)>,
}

impl PinReport {
    /// The pins that did not end up installed.
    pub fn missing(&self) -> Vec<&Pin> {
        self.results
            .iter()
            .filter(|(_, o)| matches!(o, PinOutcome::Failed(_)))
            .map(|(p, _)| p)
            .collect()
    }
}

/// What to do with one pin, decided from disk before anything is fetched.
enum Plan {
    Install(PinOutcome),
    Done(PinOutcome),
}

pub fn install_pins(
    pins: &[Pin],
    skills_dir: &Path,
    layer: Layer,
    force: bool,
    fetcher: &dyn Fetch,
) -> PinReport {
    let mut outcomes: Vec<Option<PinOutcome>> = vec![None; pins.len()];
    let mut groups: BTreeMap<(String, String), Vec<usize>> = BTreeMap::new();
    for (i, pin) in pins.iter().enumerate() {
        let url = match git_url(pin) {
            Ok(u) => u,
            Err(e) => {
                outcomes[i] = Some(PinOutcome::Failed(e));
                continue;
            }
        };
        match plan(pin, &url, skills_dir, layer, force) {
            Plan::Done(o) => outcomes[i] = Some(o),
            Plan::Install(o) => {
                outcomes[i] = Some(o);
                groups.entry((url, pin.commit.clone())).or_default().push(i);
            }
        }
    }
    for ((url, commit), members) in groups {
        let src = GitSource { url: url.clone(), git_ref: Some(commit.clone()), path: None };
        let tree = match fetcher.fetch(&src) {
            Ok(t) => t,
            Err(e) => {
                for i in members {
                    outcomes[i] = Some(PinOutcome::Failed(format!("fetching {url} at {commit}: {e}")));
                }
                continue;
            }
        };
        if tree.commit.as_deref() != Some(commit.as_str()) {
            let got = tree.commit.clone().unwrap_or_else(|| "no commit".into());
            for i in members {
                outcomes[i] = Some(PinOutcome::Failed(format!(
                    "fetched {got} from {url}, the fleet pins {commit}"
                )));
            }
            continue;
        }
        let found = match find_skills(&tree.root) {
            Ok(f) => f,
            Err(e) => {
                for i in members {
                    outcomes[i] = Some(PinOutcome::Failed(e.to_string()));
                }
                continue;
            }
        };
        for i in members {
            let pin = &pins[i];
            let mut record = |inst: &Installed| -> Result<Tracking, crate::install::InstallError> {
                let rec = SourceRecord {
                    version: RECORD_VERSION,
                    source: pin.source.clone(),
                    url: Some(url.clone()),
                    git_ref: Some(commit.clone()),
                    path: record_path_of(None, &inst.rel),
                    commit: tree.commit.clone(),
                    local: None,
                    content_hash: inst.content_hash.hex(),
                    installed_at: now_secs(),
                };
                write_record(skills_dir, inst.name.as_str(), &rec).map_err(|source| {
                    crate::install::InstallError::Io {
                        path: crate::record::record_path(skills_dir, inst.name.as_str()),
                        source,
                    }
                })?;
                Ok(Tracking::Recorded)
            };
            if let Err(e) = install_selected(
                &found,
                &Selection::Named(vec![pin.name.clone()]),
                skills_dir,
                layer,
                true,
                &mut record,
            ) {
                outcomes[i] = Some(PinOutcome::Failed(e.to_string()));
            }
        }
    }
    PinReport {
        results: pins
            .iter()
            .cloned()
            .zip(outcomes.into_iter().map(|o| o.expect("every pin has an outcome")))
            .collect(),
    }
}

/// The https URL a pin's source names (`owner/repo` or a URL).
fn git_url(pin: &Pin) -> Result<String, String> {
    match parse_source(&pin.source, Some(&pin.commit), None) {
        Ok(Source::Git(g)) if g.path.is_none() => Ok(g.url),
        Ok(Source::Git(_)) => Err(format!("{}: a pinned source names a repository, not a path in it", pin.source)),
        Ok(Source::Local(_)) => Err(format!("{}: a pinned source must be a git repository", pin.source)),
        Err(e) => Err(format!("{}: {e}", pin.source)),
    }
}

fn plan(pin: &Pin, url: &str, skills_dir: &Path, layer: Layer, force: bool) -> Plan {
    let dir = skills_dir.join(&pin.name);
    if !dir.join("SKILL.md").is_file() {
        return Plan::Install(PinOutcome::Created);
    }
    let kept = |reason| {
        if force {
            Plan::Install(PinOutcome::Replaced)
        } else {
            Plan::Done(PinOutcome::Kept(reason))
        }
    };
    let rec = match read_record(skills_dir, &pin.name) {
        Ok(Some(r)) if r.url.as_deref() == Some(url) => r,
        Ok(_) => return kept(KeptReason::OtherSource),
        Err(_) if force => return Plan::Install(PinOutcome::Replaced),
        Err(e) => return Plan::Done(PinOutcome::Failed(e.to_string())),
    };
    let current = load_skill(&dir, layer)
        .map_err(|e| e.to_string())
        .and_then(|s| s.content_hash().map_err(|e| e.to_string()));
    let current = match current {
        Ok(h) => h.hex(),
        Err(_) if force => return Plan::Install(PinOutcome::Replaced),
        Err(e) => return Plan::Done(PinOutcome::Failed(e)),
    };
    if current != rec.content_hash {
        return kept(KeptReason::LocallyModified);
    }
    if rec.commit.as_deref() == Some(pin.commit.as_str()) {
        return Plan::Done(PinOutcome::Skipped);
    }
    Plan::Install(PinOutcome::Updated { from: rec.commit })
}
```

`record_path`, `install_selected` and `find_skills` are `pub` already; `record_path_of` is `pub(crate)`. `recover_replaced` is called by `install_one` itself, so `pinned` needn't call it.

In `lib.rs`: `pub mod pinned;` and `pub use pinned::{install_pins, KeptReason, Pin, PinOutcome, PinReport};`.

- [ ] **Step 5: Run the tests**

Run: `cargo test -p rupu-skills --test it pinned:: install:: update::`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
rustfmt --edition 2021 crates/rupu-skills/src/pinned.rs crates/rupu-skills/src/install.rs crates/rupu-skills/src/lib.rs crates/rupu-skills/tests/it/pinned.rs
git add crates/rupu-skills
git commit -m "feat(skills): install skills pinned to a commit"
```

### Task A3: `PinnedCheckout` — `--skills-from` as a `Fetch`

**Files:**
- Modify: `crates/rupu-skills/src/pinned.rs` (a `#[cfg(feature = "git")] mod checkout`)
- Create: `crates/rupu-skills/tests/it/pinned_checkout.rs` (`#![cfg(feature = "git")]`); Modify: `tests/it/main.rs`

**Interfaces:**
- Produces: `rupu_skills::pinned::PinnedCheckout { pub dir: PathBuf }` implementing `Fetch` (feature `git`).
  `fetch(src)` requires `src.git_ref` (the pin), HEAD of `dir` == that commit, and no modified, staged, untracked or ignored path under `skills/`. Returns `FetchedTree::local_with_commit(dir, Some(head))`. Errors are `SourceError::Clone(<message>)`.

- [ ] **Step 1: Write the failing tests:**

```rust
#![cfg(feature = "git")]
use rupu_skills::pinned::PinnedCheckout;
use rupu_skills::{Fetch, GitSource};
use std::path::Path;

use crate::support::write_skill;

fn commit_all(repo: &git2::Repository) -> String {
    let mut idx = repo.index().unwrap();
    idx.add_all(["*"], git2::IndexAddOption::DEFAULT, None).unwrap();
    idx.write().unwrap();
    let tree = repo.find_tree(idx.write_tree().unwrap()).unwrap();
    let sig = git2::Signature::now("t", "t@example.invalid").unwrap();
    let parents: Vec<git2::Commit> = repo.head().ok().and_then(|h| h.peel_to_commit().ok()).into_iter().collect();
    let refs: Vec<&git2::Commit> = parents.iter().collect();
    repo.commit(Some("HEAD"), &sig, &sig, "c", &tree, &refs).unwrap().to_string()
}

fn checkout_with_one_skill(dir: &Path) -> (git2::Repository, String) {
    let repo = git2::Repository::init(dir).unwrap();
    write_skill(&dir.join("skills"), "work-units", "work-units", "split results");
    let c = commit_all(&repo);
    (repo, c)
}

fn src(commit: &str) -> GitSource {
    GitSource {
        url: "https://github.com/Section9Labs/rupu-skills".into(),
        git_ref: Some(commit.into()),
        path: None,
    }
}

#[test]
fn a_clean_checkout_at_the_pin_is_served() {
    let tmp = tempfile::tempdir().unwrap();
    let (_r, c) = checkout_with_one_skill(tmp.path());
    let tree = PinnedCheckout { dir: tmp.path().into() }.fetch(&src(&c)).unwrap();
    assert_eq!(tree.commit.as_deref(), Some(c.as_str()));
    assert!(tree.root.join("skills/work-units/SKILL.md").is_file());
}

#[test]
fn a_checkout_at_another_commit_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let (_r, c) = checkout_with_one_skill(tmp.path());
    let other = "0123456789abcdef0123456789abcdef01234567";
    let err = PinnedCheckout { dir: tmp.path().into() }.fetch(&src(other)).unwrap_err().to_string();
    assert!(err.contains(&c) && err.contains(other), "{err}");
}

#[test]
fn a_modified_file_under_skills_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let (_r, c) = checkout_with_one_skill(tmp.path());
    std::fs::write(tmp.path().join("skills/work-units/scripts/run.sh"), "changed\n").unwrap();
    let err = PinnedCheckout { dir: tmp.path().into() }.fetch(&src(&c)).unwrap_err().to_string();
    assert!(err.contains("skills/work-units/scripts/run.sh"), "{err}");
}

#[test]
fn an_untracked_or_ignored_file_under_skills_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let (repo, _) = checkout_with_one_skill(tmp.path());
    std::fs::write(tmp.path().join(".gitignore"), "__pycache__/\n").unwrap();
    let c = commit_all(&repo);
    let cache = tmp.path().join("skills/work-units/scripts/__pycache__");
    std::fs::create_dir_all(&cache).unwrap();
    std::fs::write(cache.join("x.pyc"), "x").unwrap();
    let err = PinnedCheckout { dir: tmp.path().into() }.fetch(&src(&c)).unwrap_err().to_string();
    assert!(err.contains("__pycache__") && err.contains("git clean -fdx skills/"), "{err}");
}

#[test]
fn a_changed_file_outside_skills_is_allowed() {
    let tmp = tempfile::tempdir().unwrap();
    let (_r, c) = checkout_with_one_skill(tmp.path());
    std::fs::write(tmp.path().join("NOTES.md"), "scratch\n").unwrap();
    PinnedCheckout { dir: tmp.path().into() }.fetch(&src(&c)).unwrap();
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p rupu-skills --features git --test it pinned_checkout::`
Expected: compile error, no `PinnedCheckout`.

- [ ] **Step 3: Implement** at the bottom of `pinned.rs`:

```rust
#[cfg(feature = "git")]
mod checkout {
    use crate::source::{Fetch, FetchedTree, GitSource, SourceError};
    use std::path::PathBuf;

    /// `rupu fleet install --skills-from <dir>`: a local checkout of the
    /// pinned commit stands in for the network. Its HEAD must be the pin and
    /// nothing under `skills/` may differ from that commit, because install
    /// copies every file in a skill's directory.
    pub struct PinnedCheckout {
        pub dir: PathBuf,
    }

    impl Fetch for PinnedCheckout {
        fn fetch(&self, src: &GitSource) -> Result<FetchedTree, SourceError> {
            let shown = self.dir.display();
            let err = |m: String| SourceError::Clone(m);
            let want = src
                .git_ref
                .as_deref()
                .ok_or_else(|| err("--skills-from serves pinned commits only".into()))?;
            let repo = git2::Repository::open(&self.dir)
                .map_err(|e| err(format!("{shown} is not a git checkout: {}", e.message())))?;
            let head = repo
                .head()
                .and_then(|h| h.peel_to_commit())
                .map_err(|e| err(format!("{shown}: {}", e.message())))?
                .id()
                .to_string();
            if head != want {
                return Err(err(format!(
                    "{shown} is at {head}; the fleet pins {want}. Run `git -C {shown} checkout {want}`"
                )));
            }
            let mut opts = git2::StatusOptions::new();
            opts.include_untracked(true)
                .recurse_untracked_dirs(true)
                .include_ignored(true)
                .recurse_ignored_dirs(true)
                .pathspec("skills");
            let statuses = repo
                .statuses(Some(&mut opts))
                .map_err(|e| err(format!("{shown}: {}", e.message())))?;
            if let Some(dirty) = statuses
                .iter()
                .find(|s| s.status() != git2::Status::CURRENT)
                .and_then(|s| s.path().map(str::to_owned))
            {
                return Err(err(format!(
                    "{shown} has files under skills/ that are not in {want} ({dirty}); run \
                     `git -C {shown} clean -fdx skills/` and `git -C {shown} checkout -- skills/`"
                )));
            }
            Ok(FetchedTree::local_with_commit(self.dir.clone(), Some(head)))
        }
    }
}

#[cfg(feature = "git")]
pub use checkout::PinnedCheckout;
```

The ignored-files test asserts the substring `git clean -fdx skills/`; the message above renders `git -C <dir> clean -fdx skills/`, which contains it.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p rupu-skills --features git --test it pinned_checkout:: pinned::`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
rustfmt --edition 2021 crates/rupu-skills/src/pinned.rs crates/rupu-skills/tests/it/pinned_checkout.rs crates/rupu-skills/tests/it/main.rs
git add crates/rupu-skills
git commit -m "feat(skills): --skills-from serves a pinned local checkout"
```

### Task A4: `rupu fleet install` installs the bundled skills; `fleet list` shows them

**Files:**
- Modify: `crates/rupu-cli/src/cmd/fleet.rs`
- Modify: `docs/cli-reference.md` (regenerated), `docs/skills.md`, `docs/engagement-profiles.md` (the "fleet" section near line 315)

**Interfaces:**
- Consumes: `rupu_coverage::builtin_profiles()`, `BundleSkill`; `rupu_skills::pinned::{install_pins, Pin, PinOutcome, KeptReason, PinnedCheckout}`; `rupu_skills::git::GitFetcher`; `rupu_skills::{Fetch, Layer}`.
- Produces: `pub fn fleet_pins() -> Vec<Pin>` (the union of the built-in bundles' skills, one per name, sorted by name); `InstallArgs.skills_from: Option<PathBuf>`.

- [ ] **Step 1: Write the failing unit test** in `fleet.rs`'s `mod tests`:

```rust
    #[test]
    fn fleet_pins_are_the_union_of_the_builtin_bundles_one_per_name() {
        let pins = fleet_pins();
        let mut names: Vec<&str> = pins.iter().map(|p| p.name.as_str()).collect();
        let len = names.len();
        names.dedup();
        assert_eq!(names.len(), len, "one pin per skill name");
        let mut want: Vec<String> = rupu_coverage::builtin_profiles()
            .into_values()
            .flat_map(|p| p.bundle.skills.into_iter().map(|s| s.name))
            .collect();
        want.sort();
        want.dedup();
        assert_eq!(pins.iter().map(|p| p.name.clone()).collect::<Vec<_>>(), want);
    }
```

Run: `cargo test -p rupu-cli --lib cmd::fleet::` — Expected: compile error, no `fleet_pins`.

- [ ] **Step 2: Implement** in `fleet.rs`:

1. `InstallArgs` gains:

```rust
    /// Install the fleet's skills from a local checkout of
    /// Section9Labs/rupu-skills at the pinned commit (offline boxes),
    /// instead of fetching them.
    #[arg(long, value_name = "DIR")]
    pub skills_from: Option<std::path::PathBuf>,
```

2. `fleet_pins`:

```rust
/// The skills the stock fleet installs: every built-in profile's bundled
/// skills, one per name (the lockstep tests keep the profiles' pins equal).
pub fn fleet_pins() -> Vec<rupu_skills::pinned::Pin> {
    let mut by_name = std::collections::BTreeMap::new();
    for (_, p) in rupu_coverage::builtin_profiles() {
        for s in p.bundle.skills {
            by_name.entry(s.name.clone()).or_insert(rupu_skills::pinned::Pin {
                name: s.name,
                source: s.source,
                tag: s.tag,
                commit: s.commit,
            });
        }
    }
    by_name.into_values().collect()
}
```

3. In `install`, after the agent/workflow tally is printed:

```rust
    let global = crate::paths::global_dir()?;
    let layer = if args.project && !same_dir(&root, &global) {
        rupu_skills::Layer::Project
    } else {
        rupu_skills::Layer::Global
    };
    let pins = fleet_pins();
    let fetcher: Box<dyn rupu_skills::Fetch> = match &args.skills_from {
        Some(dir) => Box::new(rupu_skills::pinned::PinnedCheckout { dir: dir.clone() }),
        None => Box::new(rupu_skills::git::GitFetcher),
    };
    let report = rupu_skills::pinned::install_pins(
        &pins,
        &root.join("skills"),
        layer,
        args.force,
        fetcher.as_ref(),
    );
    for (pin, outcome) in &report.results {
        println!("{}", pin_line(pin, outcome));
    }
    let missing = report.missing();
    if !missing.is_empty() {
        let names: Vec<&str> = missing.iter().map(|p| p.name.as_str()).collect();
        anyhow::bail!(
            "fleet install: {} skill(s) not installed: {}. Agents that grant them will not \
             launch. Re-run `rupu fleet install` when the network is back, or on an offline \
             box `rupu fleet install --skills-from <checkout of {} at {}>`",
            missing.len(),
            names.join(", "),
            missing[0].source,
            missing[0].commit,
        );
    }
```

with helpers:

```rust
fn same_dir(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(x), Ok(y)) => x == y,
        _ => a == b,
    }
}

fn short(c: &str) -> &str {
    &c[..c.len().min(12)]
}

fn pin_line(pin: &rupu_skills::pinned::Pin, o: &rupu_skills::pinned::PinOutcome) -> String {
    use rupu_skills::pinned::{KeptReason, PinOutcome};
    let n = &pin.name;
    match o {
        PinOutcome::Created => format!("CREATED skills/{n} ({} {})", pin.tag, short(&pin.commit)),
        PinOutcome::Skipped => format!("SKIPPED skills/{n} (pinned {})", pin.tag),
        PinOutcome::Updated { from } => format!(
            "UPDATED skills/{n} {} → {} ({})",
            from.as_deref().map(short).unwrap_or("unknown"),
            short(&pin.commit),
            pin.tag
        ),
        PinOutcome::Replaced => format!("OVERWROTE skills/{n} ({} {})", pin.tag, short(&pin.commit)),
        PinOutcome::Kept(KeptReason::LocallyModified) => {
            format!("KEPT skills/{n} (locally modified; --force replaces it)")
        }
        PinOutcome::Kept(KeptReason::OtherSource) => {
            format!("KEPT skills/{n} (installed from elsewhere; --force replaces it)")
        }
        PinOutcome::Failed(e) => format!("FAILED skills/{n}: {e}"),
    }
}
```

4. `print_list` gains, after the workflows:

```rust
    let pins = fleet_pins();
    println!("\nSkills:");
    if pins.is_empty() {
        println!("  (none)");
    }
    for p in &pins {
        println!("  {}  {} {}", p.name, p.tag, short(&p.commit));
    }
```

and its header line says `{} agents, {} workflows, {} skills`. Update the module doc comment: the fleet is agents, workflows and the bundled skills (pinned; `--skills-from`).

- [ ] **Step 3: Run the tests**

Run: `cargo test -p rupu-cli --lib cmd::fleet::`
Expected: PASS (with empty bundles, `fleet_pins()` is empty and equals the empty union).

- [ ] **Step 4: Smoke-run the binary** into a throwaway home:

```bash
cargo build -p rupu-cli
H=$(mktemp -d) && RUPU_HOME="$H" ./target/debug/rupu fleet install && RUPU_HOME="$H" ./target/debug/rupu fleet list | tail -4; rm -rf "$H"
```

Expected: the agent/workflow lines as before, then `Skills:` / `(none)`; exit 0.

- [ ] **Step 5: Docs.**
  - `python3 scripts/gen-cli-reference.py ./target/debug/rupu` regenerates `docs/cli-reference.md` (shows `--skills-from`).
  - `docs/skills.md`: a new `## Fleet skills` section after `## Updating`: the stock fleet's skills come from the built-in profiles' `[bundle]`, each pinned to a commit of `Section9Labs/rupu-skills`; `rupu fleet install` installs them into the same layer as the fleet and writes normal source records (`ref` is the commit); the outcome lines (CREATED / SKIPPED / UPDATED / OVERWROTE / KEPT / FAILED) and what each means; `rupu skill update` keeps a fleet skill at its pin, and newer skills arrive with a newer rupu and another `rupu fleet install`; offline: `git clone https://github.com/Section9Labs/rupu-skills && git -C rupu-skills checkout <commit>` on a connected machine, carry it over, `rupu fleet install --skills-from rupu-skills` (HEAD must be the pin; nothing under `skills/` may differ); a failed fetch exits 1 and names the missing skills; agents that grant a missing skill do not launch.
  - `docs/engagement-profiles.md` fleet section (near line 315): the fleet is also the bundles' skills; link to `docs/skills.md#fleet-skills`.

- [ ] **Step 6: Commit**

```bash
rustfmt --edition 2021 crates/rupu-cli/src/cmd/fleet.rs
git add crates/rupu-cli/src/cmd/fleet.rs docs/cli-reference.md docs/skills.md docs/engagement-profiles.md
git commit -m "feat(fleet): install the bundled skills at their pins; --skills-from"
```

### Task A5: lockstep tests between bundles, pins and fleet agents

**Files:**
- Modify: `crates/rupu-cli/tests/it/fleet_bundle_coverage.rs`

**Interfaces:**
- Consumes: `rupu_coverage::builtin_profiles()`, `BundleSkill`, `rupu_agent::AgentSpec::parse` (`.skills`, `.preload_skills`: `Option<Vec<String>>`), `rupu_skills::SkillName`, `FLEET_MANIFEST`.

- [ ] **Step 1: Write the tests** (append to the file):

```rust
use std::collections::{BTreeMap, BTreeSet};

fn fleet_agent_skills() -> BTreeMap<String, Vec<String>> {
    FLEET_MANIFEST
        .iter()
        .filter_map(|t| {
            let stem = t.target_relpath.strip_prefix("agents/")?.trim_end_matches(".md");
            let spec = AgentSpec::parse(t.content).expect("fleet agent parses");
            let mut skills = spec.skills.clone().unwrap_or_default();
            skills.extend(spec.preload_skills.clone().unwrap_or_default());
            Some((stem.to_string(), skills))
        })
        .collect()
}

#[test]
fn every_bundled_skill_is_well_formed_and_pinned_alike_across_profiles() {
    let mut pin_of: BTreeMap<String, (String, String, String, String)> = BTreeMap::new();
    for (id, profile) in builtin_profiles() {
        for s in &profile.bundle.skills {
            s.name
                .parse::<rupu_skills::SkillName>()
                .unwrap_or_else(|e| panic!("profile `{id}` bundles skill `{}`: {e}", s.name));
            let this = (s.name.clone(), s.source.clone(), s.tag.clone(), s.commit.clone());
            if let Some(prev) = pin_of.insert(s.name.clone(), this.clone()) {
                assert_eq!(prev, this, "profile `{id}` pins `{}` differently from another profile", s.name);
            }
        }
    }
    let per_source: BTreeMap<String, BTreeSet<String>> =
        pin_of.values().fold(BTreeMap::new(), |mut m, (_, src, _, commit)| {
            m.entry(src.clone()).or_default().insert(commit.clone());
            m
        });
    for (src, commits) in per_source {
        assert_eq!(commits.len(), 1, "every skill from `{src}` must share one pin, found {commits:?}");
    }
}

#[test]
fn fleet_agents_grant_only_bare_skill_names_their_profiles_bundle() {
    let granted = fleet_agent_skills();
    for (agent, skills) in &granted {
        for s in skills {
            s.parse::<rupu_skills::SkillName>().unwrap_or_else(|e| {
                panic!("fleet agent `{agent}` grants `{s}`: fleet agents grant bare skill names ({e})")
            });
        }
    }
    for (id, profile) in builtin_profiles() {
        let bundled: BTreeSet<&str> = profile.bundle.skills.iter().map(|s| s.name.as_str()).collect();
        for agent in &profile.bundle.agents {
            for s in granted.get(agent).into_iter().flatten() {
                assert!(
                    bundled.contains(s.as_str()),
                    "profile `{id}` bundles agent `{agent}`, which grants skill `{s}`, but `{id}`'s [bundle] skills do not include it"
                );
            }
        }
    }
}

#[test]
fn every_bundled_skill_is_granted_by_some_fleet_agent() {
    let granted: BTreeSet<String> = fleet_agent_skills().into_values().flatten().collect();
    let orphans: Vec<String> = builtin_profiles()
        .into_values()
        .flat_map(|p| p.bundle.skills.into_iter().map(|s| s.name))
        .filter(|n| !granted.contains(n))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    assert!(orphans.is_empty(), "bundled skills no fleet agent grants: {orphans:?}");
}
```

Add `rupu-skills` to `rupu-cli`'s `[dev-dependencies]` only if the test crate can't already see it (it is a normal dependency of `rupu-cli`, so `rupu_skills::` resolves in its integration tests; check with the build).

- [ ] **Step 2: Run them**

Run: `cargo test -p rupu-cli --test it fleet_bundle_coverage::`
Expected: PASS (no bundled skills and no agent grants yet; Stage C makes them bite). Then prove each one bites: temporarily add `skills: [work-units]` to `templates/fleet/agents/recon.md`'s frontmatter, run again, expect `fleet_agents_grant_only_bare_skill_names_their_profiles_bundle` to fail naming `network` and `recon`; revert.

- [ ] **Step 3: Commit**

```bash
rustfmt --edition 2021 crates/rupu-cli/tests/it/fleet_bundle_coverage.rs
git add crates/rupu-cli/tests/it/fleet_bundle_coverage.rs
git commit -m "test(fleet): bundled skills, pins and agent grants stay in lockstep"
```

### Task A6: the networked pin check, CLAUDE.md, the PR

**Files:**
- Create: `scripts/check-skill-pins.sh`
- Modify: `.github/workflows/ci.yml` (`linux-musl-build`, after "Verify it runs on a distro it was not built on")
- Modify: `CLAUDE.md` (`rupu-coverage`, `rupu-skills`, `rupu-cli` entries; "Read first" list gains the spec and this plan)

Spec §5.5 put the check in `release-beta.yml`. It goes in `ci.yml`'s musl build instead: `release-beta.yml` calls `ci.yml` before tagging, so a broken pin still blocks the release, and the check reuses the binary that job already builds rather than building another.

- [ ] **Step 1: Write the script:**

```bash
#!/usr/bin/env bash
# Install the stock fleet, pinned skills included, from GitHub into a
# throwaway RUPU_HOME, then validate every installed skill. Fails when a
# pin does not resolve or a skill does not validate. Needs network.
#
#   scripts/check-skill-pins.sh <path/to/rupu>
set -euo pipefail
rupu="$(cd "$(dirname "${1:?usage: check-skill-pins.sh <path/to/rupu>}")" && pwd)/$(basename "$1")"
home="$(mktemp -d)"
trap 'rm -rf "$home"' EXIT
export RUPU_HOME="$home"
cd "$home"
"$rupu" fleet install
"$rupu" skill validate --all
echo "check-skill-pins: ok"
```

`chmod +x scripts/check-skill-pins.sh`. Run it: `scripts/check-skill-pins.sh target/debug/rupu` — Expected: `check-skill-pins: ok` (no pins yet).

- [ ] **Step 2: Add the CI step:**

```yaml
      # Every skill the built-in profiles pin must fetch at its commit and
      # validate with this build. Needs network, so it is not a cargo test.
      - name: Check the fleet's pinned skills
        if: matrix.platform == 'linux-x64'
        run: scripts/check-skill-pins.sh target/linux-musl/release/rupu
```

- [ ] **Step 3: CLAUDE.md.** Add to "Read first": the spec and this plan. `rupu-coverage` entry: `[bundle]` carries `skills` (`BundleSkill { name, source, tag, commit }`, full-SHA commit, display-only tag). `rupu-skills` entry: `pinned` (`install_pins` over a `Fetch`, the per-skill CREATED/SKIPPED/UPDATED/REPLACED/KEPT/FAILED rule, records with `ref` = commit; `PinnedCheckout` behind `git`). `rupu-cli` entry, `rupu fleet install` sentence: it also installs the built-in bundles' skills at their pins (`fleet_pins`), `--skills-from <dir>`, exit 1 when any is missing; `scripts/check-skill-pins.sh` runs in `ci.yml`'s musl build; `tests/it/fleet_bundle_coverage.rs` also holds pins and agent grants in lockstep.

- [ ] **Step 4: Verify locally**

```bash
cargo test -p rupu-coverage --lib profile::
cargo test -p rupu-skills --features git --test it pinned:: pinned_checkout:: install:: update::
cargo test -p rupu-cli --lib cmd::fleet::
cargo test -p rupu-cli --test it fleet_bundle_coverage:: init_manifest_in_sync::
cargo clippy -p rupu-coverage -p rupu-skills -p rupu-cli --all-targets --features rupu-skills/git -- -D warnings
```

Expected: all PASS, no clippy warnings.

- [ ] **Step 5: Commit, push, open the PR**

```bash
git add scripts/check-skill-pins.sh .github/workflows/ci.yml CLAUDE.md
git commit -m "ci(fleet): check the pinned fleet skills resolve and validate"
git fetch origin main:refs/remotes/origin/main && git rebase origin/main
git push origin feat/fleet-skill-pins:feat/fleet-skill-pins
gh pr create --title "Fleet skills: pinned skills in profile bundles, fleet install --skills-from" --body "<summary of A1–A6, the spec link, the CI-placement note above, test commands run>"
```

The PR touches `.github/workflows/`, so its own CI runs; wait for it. Do not merge without matt.

---

## Stage B — the `Section9Labs/rupu-skills` repository

Work in a local directory outside the rupu checkout: `~/Code/Oracle/rupu-skills` (`git init -b main`). Everything until Task B11 is local; nothing is pushed until matt approves creating the repo.

### Task B1: repository scaffold and the scope gate

**Files:**
- Create: `README.md`, `LICENSE` (Apache-2.0 text, copied from rupu's `LICENSE`), `NOTICE`, `CHANGELOG.md`, `COMPATIBILITY.md`, `.gitignore` (`__pycache__/`, `*.pyc`, `.ruff_cache/`), `lib/scope.py`, `tests/test_scope.py`

**Interfaces:**
- Produces: `lib/scope.py` (importable as `scope` once on `sys.path`):
  - `DEFAULT_RATE = 1000`, `HARD_CEILING = 10000`
  - `class ScopeError(Exception)`
  - `load(path) -> Scope`
  - `Scope.check(target, resolve=None) -> Verdict` where `Verdict(target, ok: bool, reason: str, addrs: list[str])`
  - `Scope.excludes() -> list[str]` (deny IPs/CIDRs, for `--excludefile`)
  - `Scope.rate(requested: int | None, allow_high: bool) -> int`
  - CLI: `scope.py check <scope-file> <target>...` (one JSON line per target; exit 0 all in scope, 1 any refused, 2 usage/scope-file error), `scope.py filter <scope-file> <targets-file>` (in-scope targets to stdout, refusals to stderr; exit 1 when none in scope), `scope.py excludes <scope-file>`.

Scope rules:
- Entries: IPv4/IPv6 address, CIDR, hostname, `*.domain` (any subdomain, not the apex), `max-rate <pps>`. `!` prefix = deny; deny wins. `#` comment, blank lines ignored. Anything else is a `ScopeError` naming the line.
- A target that is an address must be inside an allow network and not inside a deny network.
- A CIDR target must be a subnet of one allow network and not entirely inside a deny network; deny networks it overlaps are handed to the scanner through `excludes()`.
- A hostname target must match an allow name or wildcard and no deny name or wildcard; it is resolved, it must resolve, and no address may be inside a deny network. If the scope has any allow networks, every address must be inside one of them. If it has none (a hostname-only scope), any address that is private, loopback, link-local, reserved or multicast is refused.
- A wildcard target (`*.x`) is refused: scan names, not patterns.

- [ ] **Step 1: Write the failing tests** `tests/test_scope.py`:

```python
import os
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "lib"))
import scope  # noqa: E402


# Documentation ranges (192.0.2.0/24, 198.51.100.0/24, 203.0.113.0/24) are not
# globally routable, so a hostname-only scope rightly refuses them. A public
# resolver's address stands in for "some public host" in those tests.
PUBLIC = "9.9.9.9"


def make(text):
    with tempfile.NamedTemporaryFile("w", suffix=".txt", delete=False) as f:
        f.write(text)
    try:
        return scope.load(f.name)
    finally:
        os.unlink(f.name)


def resolver(table):
    def resolve(name):
        if name not in table:
            raise OSError("no such host")
        return table[name]
    return resolve


class AddressTargets(unittest.TestCase):
    def setUp(self):
        self.s = make("192.0.2.0/24\n!192.0.2.13\n2001:db8::/64\n")

    def test_inside_allow(self):
        self.assertTrue(self.s.check("192.0.2.10").ok)

    def test_outside_allow(self):
        v = self.s.check("198.51.100.1")
        self.assertFalse(v.ok)
        self.assertIn("not in scope", v.reason)

    def test_deny_wins(self):
        v = self.s.check("192.0.2.13")
        self.assertFalse(v.ok)
        self.assertIn("denied", v.reason)

    def test_ipv6(self):
        self.assertTrue(self.s.check("2001:db8::5").ok)
        self.assertFalse(self.s.check("2001:db8:1::5").ok)


class CidrTargets(unittest.TestCase):
    def setUp(self):
        self.s = make("192.0.2.0/24\n!192.0.2.128/25\n")

    def test_subnet_of_allow(self):
        self.assertTrue(self.s.check("192.0.2.0/26").ok)

    def test_wider_than_allow(self):
        self.assertFalse(self.s.check("192.0.2.0/23").ok)

    def test_entirely_denied(self):
        self.assertFalse(self.s.check("192.0.2.192/26").ok)

    def test_overlapping_deny_is_allowed_and_excluded(self):
        self.assertTrue(self.s.check("192.0.2.0/24").ok)
        self.assertEqual(self.s.excludes(), ["192.0.2.128/25"])


class HostnameTargets(unittest.TestCase):
    def test_named_and_resolving_inside_networks(self):
        s = make("app.example.com\n192.0.2.0/24\n")
        v = s.check("app.example.com", resolve=resolver({"app.example.com": ["192.0.2.7"]}))
        self.assertTrue(v.ok, v.reason)
        self.assertEqual(v.addrs, ["192.0.2.7"])

    def test_named_but_resolving_outside_networks(self):
        s = make("app.example.com\n192.0.2.0/24\n")
        v = s.check("app.example.com", resolve=resolver({"app.example.com": ["192.0.2.7", "203.0.113.9"]}))
        self.assertFalse(v.ok)
        self.assertIn("203.0.113.9", v.reason)

    def test_hostname_only_scope_refuses_a_private_address(self):
        s = make("app.example.com\n")
        v = s.check("app.example.com", resolve=resolver({"app.example.com": ["10.0.0.5"]}))
        self.assertFalse(v.ok)
        self.assertIn("10.0.0.5", v.reason)

    def test_hostname_only_scope_allows_a_public_address(self):
        s = make("app.example.com\n")
        self.assertTrue(s.check("app.example.com", resolve=resolver({"app.example.com": [PUBLIC]})).ok)

    def test_hostname_only_scope_refuses_a_documentation_address(self):
        s = make("app.example.com\n")
        self.assertFalse(s.check("app.example.com", resolve=resolver({"app.example.com": ["203.0.113.9"]})).ok)

    def test_a_hostname_named_max_rate_is_a_hostname(self):
        s = make("max-rate.example.com\n")
        self.assertIsNone(s.max_rate)
        self.assertTrue(s.check("max-rate.example.com", resolve=resolver({"max-rate.example.com": [PUBLIC]})).ok)

    def test_wildcard_covers_subdomains_not_the_apex(self):
        s = make("*.example.com\n")
        r = resolver({"a.example.com": [PUBLIC], "example.com": [PUBLIC]})
        self.assertTrue(s.check("a.example.com", resolve=r).ok)
        self.assertFalse(s.check("example.com", resolve=r).ok)

    def test_denied_name(self):
        s = make("*.example.com\n!admin.example.com\n")
        self.assertFalse(s.check("admin.example.com", resolve=resolver({"admin.example.com": [PUBLIC]})).ok)

    def test_address_in_deny_network(self):
        s = make("*.example.com\n!203.0.113.0/24\n")
        self.assertFalse(s.check("a.example.com", resolve=resolver({"a.example.com": ["203.0.113.1"]})).ok)

    def test_unresolvable(self):
        s = make("app.example.com\n")
        v = s.check("app.example.com", resolve=resolver({}))
        self.assertFalse(v.ok)
        self.assertIn("does not resolve", v.reason)

    def test_case_and_trailing_dot(self):
        s = make("App.Example.com.\n")
        self.assertTrue(s.check("app.example.COM", resolve=resolver({"app.example.com": [PUBLIC]})).ok)

    def test_wildcard_target_refused(self):
        self.assertFalse(make("*.example.com\n").check("*.example.com").ok)


class ScopeFile(unittest.TestCase):
    def test_bad_line_is_an_error_naming_it(self):
        with self.assertRaises(scope.ScopeError) as cm:
            make("192.0.2.0/24\nnot a target!\n")
        self.assertIn("line 2", str(cm.exception))

    def test_comments_and_blanks(self):
        s = make("# engagement 42\n\n192.0.2.1  # web\n")
        self.assertTrue(s.check("192.0.2.1").ok)

    def test_empty_scope_refuses_everything(self):
        self.assertFalse(make("# nothing\n").check("192.0.2.1").ok)


class Rates(unittest.TestCase):
    def test_default(self):
        self.assertEqual(make("192.0.2.1\n").rate(None, False), scope.DEFAULT_RATE)

    def test_up_to_ceiling(self):
        self.assertEqual(make("192.0.2.1\n").rate(10000, False), 10000)

    def test_above_ceiling_needs_flag_and_max_rate(self):
        s = make("192.0.2.1\n")
        with self.assertRaises(scope.ScopeError):
            s.rate(20000, True)
        s2 = make("192.0.2.1\nmax-rate 50000\n")
        with self.assertRaises(scope.ScopeError):
            s2.rate(20000, False)
        self.assertEqual(s2.rate(20000, True), 20000)
        with self.assertRaises(scope.ScopeError):
            s2.rate(60000, True)

    def test_non_positive(self):
        with self.assertRaises(scope.ScopeError):
            make("192.0.2.1\n").rate(0, False)


if __name__ == "__main__":
    unittest.main()
```

- [ ] **Step 2: Run them to see them fail**

Run: `python3 -m unittest discover -s tests -v`
Expected: `ModuleNotFoundError: No module named 'scope'`.

- [ ] **Step 3: Write `lib/scope.py`:**

```python
"""Engagement scope gate. Every script calls this before it touches a target.

Scope file, one entry per line:
  192.0.2.10          an address
  192.0.2.0/24        a network (IPv4 or IPv6)
  app.example.com     a hostname
  *.example.com       any subdomain of example.com (not example.com itself)
  !192.0.2.13         deny: any entry prefixed with "!"; deny always wins
  max-rate 50000      raise the rate ceiling for this engagement
  # comment           blank lines and comments are ignored
"""
import ipaddress
import json
import re
import socket
import sys
from dataclasses import dataclass, field

DEFAULT_RATE = 1000
HARD_CEILING = 10000
_HOST = re.compile(r"^(?=.{1,253}$)([a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?)(\.[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?)*$")


class ScopeError(Exception):
    pass


@dataclass
class Verdict:
    target: str
    ok: bool
    reason: str
    addrs: list = field(default_factory=list)


def _norm_name(s):
    return s.strip().lower().rstrip(".")


def _system_resolve(name):
    infos = socket.getaddrinfo(name, None, proto=socket.IPPROTO_TCP)
    return sorted({i[4][0] for i in infos})


@dataclass
class Scope:
    allow_nets: list = field(default_factory=list)
    deny_nets: list = field(default_factory=list)
    allow_names: set = field(default_factory=set)
    deny_names: set = field(default_factory=set)
    allow_wild: list = field(default_factory=list)
    deny_wild: list = field(default_factory=list)
    max_rate: int = None

    def _name_matches(self, name, names, wild):
        return name in names or any(name.endswith("." + w) for w in wild)

    def _check_addr(self, addr):
        if any(addr in n for n in self.deny_nets):
            return f"{addr} is denied by the scope"
        if not any(addr in n for n in self.allow_nets):
            return f"{addr} is not in scope"
        return None

    def check(self, target, resolve=None):
        t = target.strip()
        if t.startswith("*"):
            return Verdict(target, False, "a wildcard is a scope entry, not a target; scan names")
        try:
            net = ipaddress.ip_network(t, strict=False)
        except ValueError:
            net = None
        if net is not None and "/" not in t:
            why = self._check_addr(net.network_address)
            return Verdict(target, why is None, why or "in scope", [t])
        if net is not None:
            if any(net.version == d.version and net.subnet_of(d) for d in self.deny_nets):
                return Verdict(target, False, f"{t} is entirely denied by the scope")
            if not any(net.version == a.version and net.subnet_of(a) for a in self.allow_nets):
                return Verdict(target, False, f"{t} is not inside a scope network")
            return Verdict(target, True, "in scope", [str(net)])
        name = _norm_name(t)
        if not _HOST.match(name):
            return Verdict(target, False, f"{target!r} is not an address, network or hostname")
        if self._name_matches(name, self.deny_names, self.deny_wild):
            return Verdict(target, False, f"{name} is denied by the scope")
        if not self._name_matches(name, self.allow_names, self.allow_wild):
            return Verdict(target, False, f"{name} is not in scope")
        try:
            addrs = (resolve or _system_resolve)(name)
        except OSError as e:
            return Verdict(target, False, f"{name} does not resolve ({e})")
        if not addrs:
            return Verdict(target, False, f"{name} does not resolve")
        for a in addrs:
            ip = ipaddress.ip_address(a.split("%", 1)[0])
            if any(ip in n for n in self.deny_nets):
                return Verdict(target, False, f"{name} resolves to {a}, which the scope denies", addrs)
            if self.allow_nets:
                if not any(ip in n for n in self.allow_nets):
                    return Verdict(target, False, f"{name} resolves to {a}, outside the scope networks", addrs)
            elif not ip.is_global:
                return Verdict(
                    target, False,
                    f"{name} resolves to {a}, a non-public address; add its network to the scope to allow it",
                    addrs,
                )
        return Verdict(target, True, "in scope", addrs)

    def excludes(self):
        return [str(n) for n in self.deny_nets]

    def rate(self, requested, allow_high):
        r = DEFAULT_RATE if requested is None else int(requested)
        if r <= 0:
            raise ScopeError("rate must be a positive number of packets per second")
        if r <= HARD_CEILING:
            return r
        if not allow_high:
            raise ScopeError(f"rate {r} is above the {HARD_CEILING} pps ceiling; pass --allow-high-rate")
        if self.max_rate is None or r > self.max_rate:
            raise ScopeError(f"rate {r} is above the ceiling and the scope file's max-rate ({self.max_rate})")
        return r


def load(path):
    s = Scope()
    with open(path, encoding="utf-8") as f:
        for n, raw in enumerate(f, 1):
            line = raw.split("#", 1)[0].strip()
            if not line:
                continue
            parts = line.split()
            if parts[0] == "max-rate":
                if len(parts) != 2 or not parts[1].isdigit() or int(parts[1]) <= 0:
                    raise ScopeError(f"line {n}: max-rate takes one positive number")
                s.max_rate = int(parts[1])
                continue
            deny = line.startswith("!")
            entry = line[1:].strip() if deny else line
            try:
                net = ipaddress.ip_network(entry, strict=False)
            except ValueError:
                net = None
            if net is not None:
                (s.deny_nets if deny else s.allow_nets).append(net)
                continue
            if entry.startswith("*."):
                base = _norm_name(entry[2:])
                if _HOST.match(base):
                    (s.deny_wild if deny else s.allow_wild).append(base)
                    continue
            else:
                name = _norm_name(entry)
                if _HOST.match(name):
                    (s.deny_names if deny else s.allow_names).add(name)
                    continue
            raise ScopeError(f"line {n}: {raw.strip()!r} is not an address, network, hostname or *.domain")
    return s


def _main(argv):
    if len(argv) < 2 or argv[0] not in ("check", "filter", "excludes"):
        print("usage: scope.py check <scope-file> <target>... | filter <scope-file> <targets-file> | excludes <scope-file>", file=sys.stderr)
        return 2
    try:
        s = load(argv[1])
    except (OSError, ScopeError) as e:
        print(f"scope: {e}", file=sys.stderr)
        return 2
    if argv[0] == "excludes":
        print("\n".join(s.excludes()))
        return 0
    if argv[0] == "check":
        targets = argv[2:]
        if not targets:
            print("scope: no targets given", file=sys.stderr)
            return 2
    else:
        if len(argv) != 3:
            print("usage: scope.py filter <scope-file> <targets-file>", file=sys.stderr)
            return 2
        with open(argv[2], encoding="utf-8") as f:
            targets = [ln.split("#", 1)[0].strip() for ln in f if ln.split("#", 1)[0].strip()]
    ok_any, bad_any = False, False
    for t in targets:
        v = s.check(t)
        if argv[0] == "check":
            print(json.dumps(v.__dict__))
        elif v.ok:
            print(t)
        else:
            print(f"refused {t}: {v.reason}", file=sys.stderr)
        ok_any |= v.ok
        bad_any |= not v.ok
    if argv[0] == "check":
        return 1 if bad_any else 0
    return 0 if ok_any else 1


if __name__ == "__main__":
    sys.exit(_main(sys.argv[1:]))
```

- [ ] **Step 4: Run the tests**

Run: `python3 -m unittest discover -s tests -v`
Expected: PASS. Then by hand: `printf '192.0.2.0/24\n' > /tmp/s && python3 lib/scope.py check /tmp/s 192.0.2.1 198.51.100.1; echo $?` → two JSON lines, exit 1.

- [ ] **Step 5: Write the repo docs.**
  - `README.md`: what the repo is (Agent Skills in the agentskills.io `SKILL.md` format for authorized security assessments); the skill table (name, one line, the tools it drives); "Authorized use only: every scanning script refuses targets outside the scope file you give it"; installing with rupu (`rupu skill install Section9Labs/rupu-skills --ref vX.Y.Z --all`, or `rupu fleet install`, which pins them) and with other clients (copy `skills/<name>` into the client's skills directory); scope-file format (from the `scope.py` docstring); running the tests; licence.
  - `NOTICE`: `rupu-skills — Copyright 2026 Section9Labs. Licensed under the Apache License 2.0.` plus a section per upstream filled in by Tasks B4 and B6–B7 (repo URL, commit, licence, which files).
  - `CHANGELOG.md`: `## Unreleased`.
  - `COMPATIBILITY.md`: a table `| rupu-skills | tested with rupu | notes |`, empty row for `v1.0.0` filled at release.

- [ ] **Step 6: Commit**

```bash
git add -A && git commit -m "Scaffold rupu-skills and the scope gate"
```

### Task B2: the preflight

**Files:**
- Create: `lib/preflight.py`, `tests/test_preflight.py`

**Interfaces:**
- Produces: `lib/preflight.py`:
  - `TOOLS: dict[str, Tool]` with `Tool(binaries: list[str], apt: str | None, fallback: str | None)`; at least: `nmap` (`nmap`, apt `nmap`), `masscan` (`masscan`, apt `masscan`), `naabu` (`naabu`, apt `naabu`, fallback `go install -v github.com/projectdiscovery/naabu/v2/cmd/naabu@latest`), `httpx` (binaries `httpx-toolkit`, `httpx` — Kali packages ProjectDiscovery's httpx as `httpx-toolkit`; apt `httpx-toolkit`, fallback `go install -v github.com/projectdiscovery/httpx/cmd/httpx@latest`), `nuclei` (apt `nuclei`, fallback go install), `testssl` (`testssl`, `testssl.sh`; apt `testssl.sh`), `sslscan`, `enum4linux-ng`, `netexec` (`nxc`, `netexec`; apt `netexec`, fallback `pipx install git+https://github.com/Pennyw0rth/NetExec`), `ldapsearch` (apt `ldap-utils`), `onesixtyone`, `snmpwalk` (apt `snmp`).
  - `find(tool) -> str | None` (the first binary on `PATH`)
  - `raw_sockets(binary_path) -> (bool, str)`: root; or on Linux, `cap_net_raw` in `getcap <binary>` output or bit 13 of `CapEff` in `/proc/self/status`; otherwise `(False, reason)`.
  - `can_install() -> bool`: root, or `sudo -n true` exits 0.
  - CLI: `preflight.py check <tool>...` → JSON `{tool: {"path", "install"}}`, exit 1 when any is missing; `preflight.py raw <tool>` → JSON `{"raw": bool, "reason": str, "privileged_flag": "--privileged" | null}` (nmap with file capabilities needs `--privileged`); `preflight.py install <tool>... --install` runs `apt-get install -y` (via `sudo -n` when not root) only for tools with an `apt` package, only when `can_install()`; without `--install` it prints the commands it would run and exits 1.

- [ ] **Step 1: Write the failing tests** `tests/test_preflight.py`:

```python
import os
import stat
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

LIB = Path(__file__).resolve().parent.parent / "lib"
sys.path.insert(0, str(LIB))
import preflight  # noqa: E402


class Find(unittest.TestCase):
    def test_finds_the_first_binary_on_path(self):
        with tempfile.TemporaryDirectory() as d:
            b = Path(d) / "httpx-toolkit"
            b.write_text("#!/bin/sh\n")
            b.chmod(b.stat().st_mode | stat.S_IEXEC)
            with mock.patch.dict(os.environ, {"PATH": d}):
                self.assertEqual(preflight.find("httpx"), str(b))

    def test_missing(self):
        with tempfile.TemporaryDirectory() as d, mock.patch.dict(os.environ, {"PATH": d}):
            self.assertIsNone(preflight.find("nmap"))


class Raw(unittest.TestCase):
    def test_root_has_raw_sockets(self):
        with mock.patch.object(preflight.os, "geteuid", return_value=0):
            self.assertTrue(preflight.raw_sockets("/usr/bin/nmap")[0])

    def test_unprivileged_without_caps(self):
        with mock.patch.object(preflight.os, "geteuid", return_value=1000), \
             mock.patch.object(preflight, "_file_caps", return_value=""), \
             mock.patch.object(preflight, "_proc_capeff", return_value=0):
            ok, reason = preflight.raw_sockets("/usr/bin/nmap")
            self.assertFalse(ok)
            self.assertIn("CAP_NET_RAW", reason)

    def test_file_capability(self):
        with mock.patch.object(preflight.os, "geteuid", return_value=1000), \
             mock.patch.object(preflight, "_file_caps", return_value="/usr/bin/nmap cap_net_raw,cap_net_admin=eip"), \
             mock.patch.object(preflight, "_proc_capeff", return_value=0):
            self.assertTrue(preflight.raw_sockets("/usr/bin/nmap")[0])


class Cli(unittest.TestCase):
    def run_cli(self, *args, path):
        env = dict(os.environ, PATH=path)
        return subprocess.run([sys.executable, "-I", str(LIB / "preflight.py"), *args],
                              capture_output=True, text=True, env=env)

    def test_check_reports_missing_with_install_hint(self):
        with tempfile.TemporaryDirectory() as d:
            r = self.run_cli("check", "naabu", path=d)
            self.assertEqual(r.returncode, 1)
            self.assertIn("apt-get install -y naabu", r.stdout)
            self.assertIn("go install", r.stdout)

    def test_install_without_flag_only_prints(self):
        with tempfile.TemporaryDirectory() as d:
            r = self.run_cli("install", "nmap", path=d)
            self.assertEqual(r.returncode, 1)
            self.assertIn("apt-get install -y nmap", r.stdout + r.stderr)

    def test_unknown_tool(self):
        with tempfile.TemporaryDirectory() as d:
            r = self.run_cli("check", "no-such-tool", path=d)
            self.assertEqual(r.returncode, 2)


if __name__ == "__main__":
    unittest.main()
```

- [ ] **Step 2: Run to see them fail** — `python3 -m unittest tests.test_preflight -v` → `ModuleNotFoundError`.

- [ ] **Step 3: Write `lib/preflight.py`** to the interface above. Helpers `_file_caps(path) -> str` (output of `getcap <path>`, `""` when `getcap` is missing or fails) and `_proc_capeff() -> int` (hex `CapEff:` from `/proc/self/status`, `0` off Linux) are the seams the tests patch. `raw_sockets` returns `"--privileged"` as the flag hint when the capability comes from file caps (nmap needs it to use them unprivileged). Every subprocess call has a timeout (10 s) and never uses a shell.

- [ ] **Step 4: Run the tests** — `python3 -m unittest discover -s tests -v` → PASS.

- [ ] **Step 5: Commit** — `git add -A && git commit -m "Add the tool and capability preflight"`.

### Task B3: sync, check, CI

**Files:**
- Create: `Makefile`, `tools/sync_lib.py`, `tools/check_layout.py`, `.github/workflows/ci.yml`, `tests/requirements.txt` (`jsonschema==4.23.0`, `ruff==0.6.9`)

**Interfaces:**
- `make sync`: copy `lib/*.py` into every `skills/*/scripts/_lib/` that exists (a skill opts in by having `scripts/`).
- `make check`: `tools/sync_lib.py --check` (exit 1 naming any `_lib` copy that differs from `lib/`) and `tools/check_layout.py`: every `skills/<d>/SKILL.md` has frontmatter whose keys ⊆ {name, description, license, compatibility, metadata}, `name == <d>`, `metadata` values are strings; no `SKILL.md` anywhere outside `skills/<d>/SKILL.md` (Review Focus 5); every script under `skills/*/scripts/*.py` that is not in `_lib/` contains `scope` usage (`from scope import` or `import scope`) unless listed in `tools/check_layout.py`'s `NO_TARGETS` set (scripts that never touch a network target, e.g. parsers).
- `make test`: `python3 -m unittest discover -s tests -v`.

- [ ] **Step 1: Write `tools/check_layout.py` and a test for it** (`tests/test_layout.py`) that builds a temp tree with (a) a valid skill, (b) a `name` mismatch, (c) an extra frontmatter key `version`, (d) a stray `tests/fixtures/x/SKILL.md`, (e) a scanning script without a scope import — and asserts each of (b)–(e) is reported with its path, and (a) alone passes. Frontmatter parsing: split on the first two `---` lines and parse the YAML subset by hand (`key: value`, `metadata:` with indented `k: v` lines); a value the subset can't read is reported, never ignored.

- [ ] **Step 2: Run, see it fail, implement, run, see it pass** — `python3 -m unittest tests.test_layout -v`.

- [ ] **Step 3: Write `tools/sync_lib.py`** (copy mode and `--check` mode) and the `Makefile`:

```make
.PHONY: sync check test lint
sync:
	python3 tools/sync_lib.py
check:
	python3 tools/sync_lib.py --check
	python3 tools/check_layout.py
test:
	python3 -m unittest discover -s tests -v
lint:
	ruff check lib tools tests skills
```

- [ ] **Step 4: Write `.github/workflows/ci.yml`:**

```yaml
name: ci
on:
  pull_request:
  push:
    branches: [main]
permissions:
  contents: read
jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v5
      - uses: actions/setup-python@v5
        with:
          python-version: "3.12"
      - run: pip install -r tests/requirements.txt
      - run: make lint
      - run: make check
      - run: make test
      - name: shellcheck
        run: |
          files=$(git ls-files '*.sh')
          [ -z "$files" ] || shellcheck $files
      - name: agentskills.io reference validator
        run: |
          pip install "git+https://github.com/agentskills/agentskills@main#subdirectory=skills-ref"
          for d in skills/*/; do skills-ref validate "$d"; done
      - name: rupu skill validate
        env:
          GH_TOKEN: ${{ github.token }}
        run: |
          # The newest release, prereleases included: a stable release can
          # predate `rupu skill`.
          tag=$(gh release list --repo Section9Labs/rupu --limit 1 --json tagName -q '.[0].tagName')
          gh release download "$tag" --repo Section9Labs/rupu --pattern 'rupu-linux-x64*.tar.gz' --dir /tmp/rupu
          tar -xzf /tmp/rupu/rupu-linux-x64*.tar.gz -C /tmp/rupu
          rupu=$(find /tmp/rupu -type f -name rupu -perm -u+x | head -1)
          export RUPU_HOME=$(mktemp -d)
          "$rupu" skill install "$PWD" --all
          "$rupu" skill validate --all
```

Before committing, check three things against reality and fix the YAML to match: the rupu release asset name (`gh release view --repo Section9Labs/rupu --json assets -q '.assets[].name'`), the `skills-ref` install path and CLI (read `https://github.com/agentskills/agentskills/tree/main/skills-ref` README; pin a commit instead of `@main` once known), and that `rupu skill install <dir> --all` is the right flag spelling (`rupu skill install --help`).

- [ ] **Step 5: Run locally** — `make lint check test` → PASS.

- [ ] **Step 6: Commit** — `git add -A && git commit -m "Add sync, layout check and CI"`.

### Task B4: vendor `kali-linux`

**Files:**
- Create: `skills/kali-linux/` (upstream files unchanged), `skills/kali-linux/UPSTREAM`; Modify: `NOTICE`

- [ ] **Step 1:** In a fresh empty directory under the scratchpad, `git clone --depth 1 https://github.com/TerminalSkills/skills ts && git -C ts rev-parse HEAD`. Read `ts/LICENSE` (confirm Apache-2.0) and every file in `ts/skills/kali-linux/` in full.
- [ ] **Step 2:** Copy `ts/skills/kali-linux/` to `skills/kali-linux/` unchanged. Copy `ts/LICENSE` to `skills/kali-linux/LICENSE`. Write `skills/kali-linux/UPSTREAM`: `repository: https://github.com/TerminalSkills/skills`, `path: skills/kali-linux`, `commit: <sha>`, `license: Apache-2.0`, `changes: none`.
- [ ] **Step 3:** `make check`. If the frontmatter has a key outside the allowed set or a list-valued `metadata` entry (research noted `metadata.tags` is a list), do NOT edit the upstream file: add `kali-linux` to an `UPSTREAM_EXEMPT` set in `check_layout.py` that skips only the metadata-type check, and record it in `UPSTREAM` under `known deviations`. Run `skills-ref validate skills/kali-linux` locally if installed; if the reference validator rejects it, stop and tell matt (vendoring it modified changes the attribution story).
- [ ] **Step 4:** `NOTICE` gains the TerminalSkills section.
- [ ] **Step 5: Commit** — `git add -A && git commit -m "Vendor kali-linux from TerminalSkills/skills"`.

### Task B5: the Mythos authoring setup

**Files:** (scratchpad only, never committed to either repository)
- Create: `<scratchpad>/author/.rupu/agents/skill-author.md`
- Create: `<scratchpad>/author/briefs/<skill>.md` per skill (Tasks B6–B9 give each brief's content)

- [ ] **Step 1: Write the agent:**

```markdown
---
name: skill-author
description: Drafts one Agent Skill (agentskills.io SKILL.md format) for authorized security assessments, from a brief.
provider: anthropic
auth: api-key
model: claude-mythos-5
permissionMode: ask
maxTurns: 80
tools: ["read_file", "write_file", "edit_file", "glob", "grep", "bash"]
---
You write Agent Skills for the public, Apache-2.0 repository Section9Labs/rupu-skills, used by authorized security assessors.

Hard rules:
- Write from public methodology and public tool documentation only. Never invent client names, real organisations, or real-looking assessment data. Example addresses come from 192.0.2.0/24, 198.51.100.0/24 and 203.0.113.0/24; example names from example.com / example.net.
- Frontmatter keys: only name, description, license (Apache-2.0), compatibility, metadata (string values only). name equals the directory name. description says what the skill does AND when to use it, under 1024 characters.
- SKILL.md under 500 lines; depth goes in references/.
- Scripts: Python 3 standard library only. Every script that touches a network target imports the scope gate (`sys.path.insert(0, str(Path(__file__).resolve().parent / "_lib")); import scope`) and refuses out-of-scope targets BEFORE running any tool. Never use shell=True. Every subprocess has a timeout. Rates go through scope.rate().
- Missing tools: call preflight (`import preflight`) and either fall back or exit 2 with the install hint. Never install anything unless the operator passed --install.
- Client-agnostic: the SKILL.md must make sense to any agent. rupu-specific instructions go only in references/rupu.md.
- Make the tests in tests/ for this skill pass; do not edit them.

The brief for the skill you are writing follows in the task prompt. Work in the checkout you were started in; write only under skills/<name>/ and tests/test_<name>*.py is read-only for you.
```

- [ ] **Step 2: Check the agent loads** — from `<scratchpad>/author`: `rupu agent validate skill-author` → ok.
- [ ] **Step 3:** For each skill task, the run is: copy the rupu-skills working tree into `<scratchpad>/author/work/` (a fresh copy per skill), write the brief, `rupu run skill-author "$(cat briefs/<skill>.md)"` in the ask permission mode (check `rupu run --help` for the flag; the agent's `permissionMode: ask` already sets it) from `<scratchpad>/author/work`, approve only writes under `skills/<name>/`, then review (Step "Review" of each task) and copy the reviewed files into the real repo.

### Task B6: `network-recon`

**Files:**
- Create: `skills/network-recon/SKILL.md`, `references/{timing-and-rates.md,nse-choices.md,ipv6.md,handoff.md,rupu.md}`, `scripts/{discover.py,ports.py,services.py,parse.py}`, `scripts/_lib/` (via `make sync`)
- Create: `tests/test_network_recon.py`, `tests/fixtures/nmap-services.xml`, `tests/fixtures/naabu.jsonl`, `tests/fixtures/fake-bin/{nmap,masscan,naabu}` (shell shims)

**Interfaces:**
- `discover.py --scope <file> [--targets <file> | <target>...] [--rate N] [--allow-high-rate] [--engine masscan|naabu|nmap] --out <dir>` → writes `<out>/live-hosts.txt`; refuses (exit 1, no tool run) when any given target is out of scope; with no raw sockets, `masscan` is unavailable and it picks `naabu` connect mode or `nmap -sn -PS`/`-PA` TCP pings.
- `ports.py --scope <file> --hosts <live-hosts.txt> --phase top|all|udp [--rate N] --out <dir>` → `<out>/ports-<phase>.{xml,gnmap,nmap}` via nmap `-oA` (top: `--top-ports 1000`; all: `-p-`; udp: `-sU -p 53,67,69,123,137,161,500,623,1900,5353`); `-sS` with raw sockets, `-sT` without; `--excludefile` from `scope.py excludes`; `--max-rate` from `scope.rate`.
- `services.py --scope <file> --from <ports-*.xml>... --out <dir>` → `nmap -sV --version-intensity 7 -sC` restricted to `--script "default and safe"` on the open ports only; `-oA <out>/services`.
- `parse.py <nmap.xml|naabu.jsonl>... [-o out.json]` → JSON array of `{"host","hostname","port","proto","state","service","product","version","extrainfo","tunnel"}` for open ports, sorted by host then port; no network access, in `NO_TARGETS`.

- [ ] **Step 1: Write the fixtures.** `tests/fixtures/nmap-services.xml`: a hand-written nmap XML (nmaprun → two `<host>`s, 192.0.2.10 with 22/tcp ssh OpenSSH 9.2p1, 443/tcp https nginx 1.24.0 with `tunnel="ssl"`, 161/udp snmp; 192.0.2.20 with 445/tcp microsoft-ds Samba smbd 4.x, 389/tcp ldap OpenLDAP 2.6, and 8080/tcp `state="closed"`). `naabu.jsonl`: two lines `{"host":"192.0.2.30","ip":"192.0.2.30","port":80,"protocol":"tcp"}` and port 3306. `fake-bin/nmap` etc.: `#!/bin/sh` scripts that append `"$0 $*"` to `$FAKE_LOG` and exit 0 (and, for nmap with `-oA <p>`, write an empty `<p>.xml` containing `<nmaprun></nmaprun>`).

- [ ] **Step 2: Write the tests** `tests/test_network_recon.py`:

```python
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SCRIPTS = ROOT / "skills/network-recon/scripts"
FIX = ROOT / "tests/fixtures"


def run(script, *args, env_extra=None):
    env = dict(os.environ, PATH=f"{FIX / 'fake-bin'}:/usr/bin:/bin", **(env_extra or {}))
    return subprocess.run([sys.executable, "-I", str(SCRIPTS / script), *args],
                          capture_output=True, text=True, env=env, timeout=60)


class Parse(unittest.TestCase):
    def test_nmap_xml_open_ports_only(self):
        r = run("parse.py", str(FIX / "nmap-services.xml"))
        self.assertEqual(r.returncode, 0, r.stderr)
        rows = json.loads(r.stdout)
        ports = [(x["host"], x["port"], x["proto"]) for x in rows]
        self.assertIn(("192.0.2.10", 443, "tcp"), ports)
        self.assertIn(("192.0.2.10", 161, "udp"), ports)
        self.assertNotIn(("192.0.2.20", 8080, "tcp"), ports)
        https = next(x for x in rows if x["port"] == 443)
        self.assertEqual(https["tunnel"], "ssl")
        self.assertEqual(https["product"], "nginx")

    def test_naabu_jsonl(self):
        rows = json.loads(run("parse.py", str(FIX / "naabu.jsonl")).stdout)
        self.assertEqual([(x["host"], x["port"]) for x in rows], [("192.0.2.30", 80), ("192.0.2.30", 3306)])


class ScopeGate(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.scope = Path(self.tmp.name) / "scope.txt"
        self.scope.write_text("192.0.2.0/24\n")
        self.log = Path(self.tmp.name) / "fake.log"

    def tearDown(self):
        self.tmp.cleanup()

    def assert_refused_without_running(self, script, *args):
        r = run(script, "--scope", str(self.scope), *args, "--out", self.tmp.name,
                env_extra={"FAKE_LOG": str(self.log)})
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("198.51.100.1", r.stderr)
        self.assertFalse(self.log.exists() and self.log.read_text().strip(),
                         f"a scanner ran: {self.log.read_text() if self.log.exists() else ''}")

    def test_discover_refuses_an_out_of_scope_target(self):
        self.assert_refused_without_running("discover.py", "192.0.2.0/28", "198.51.100.1")

    def test_ports_refuses_an_out_of_scope_host(self):
        hosts = Path(self.tmp.name) / "live.txt"
        hosts.write_text("192.0.2.5\n198.51.100.1\n")
        self.assert_refused_without_running("ports.py", "--hosts", str(hosts), "--phase", "top")

    def test_rate_above_ceiling_is_refused(self):
        r = run("discover.py", "--scope", str(self.scope), "192.0.2.0/28", "--rate", "20000",
                "--out", self.tmp.name, env_extra={"FAKE_LOG": str(self.log)})
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("ceiling", r.stderr)

    def test_in_scope_discover_runs_a_scanner_with_the_capped_rate(self):
        r = run("discover.py", "--scope", str(self.scope), "192.0.2.0/28", "--engine", "naabu",
                "--out", self.tmp.name, env_extra={"FAKE_LOG": str(self.log)})
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn("naabu", self.log.read_text())
        self.assertIn("1000", self.log.read_text())


if __name__ == "__main__":
    unittest.main()
```

- [ ] **Step 3: Run them to see them fail** — `python3 -m unittest tests.test_network_recon -v` → errors (scripts missing).

- [ ] **Step 4: Brief for Mythos** (`briefs/network-recon.md`): the interface block above verbatim; spec §7's `network-recon` paragraph; methodology the SKILL.md must teach — confirm scope and rules of engagement first; preflight; discovery (masscan `--rate`, naabu `-rate`, both with excludes; nmap `-sn` fallback with TCP SYN/ACK pings when ICMP is filtered; `-Pn` only on hosts the operator says are up); phased ports (top 1000 → all TCP on live hosts → curated UDP, explaining why UDP is slow and why the list is short); service detection (`-sV`, `default and safe` NSE only; never `intrusive`, `dos`, `brute`, `exploit` in recon); output discipline (always `-oA`, keep raw output as evidence); timing (`-T3` default, `-T4` on a LAN, never `-T5`; `--max-retries`, `--host-timeout`); IPv6 (no sweeping, scan known addresses); handoff table (service → tools: http/https → httpx then nuclei; tls → testssl.sh / sslscan; smb → enum4linux-ng / netexec null+guest; ldap → anonymous ldapsearch; snmp → onesixtyone / snmpwalk; ssh → ssh-audit-style version and auth-method checks; databases → version and auth-method checks only); `references/rupu.md` (call `assets.mark` with `kind: "network:host"` at `discovered` per live host, `network:service` at `enumerated` per open port, label `<host>:<port> <service>/<version>`; hand `parse.py` output to `work-units`). Public sources it may draw on: nmap.org book chapters on host discovery, port scanning techniques, timing, NSE categories; masscan and naabu READMEs; the adapted x-glacier nmap/masscan/naabu references and Decepticon `active-recon` rate/phase sections (fetch them into the work copy; credit them in SKILL.md `metadata.source` and NOTICE).

- [ ] **Step 5: Run Mythos** per Task B5 Step 3, then `make sync`.

- [ ] **Step 6: Review.** Read every line. Check: every command line is correct for current nmap/masscan/naabu (`man nmap`, `masscan --help`, `naabu -h` on kali6); no script runs a tool before `scope.check` on every target; no `shell=True`; timeouts present; rates go through `scope.rate`; deny entries become `--excludefile` (nmap/masscan) or `-exclude-hosts`/`-ef` (naabu); fallbacks really happen without raw sockets; the SKILL.md is client-agnostic; adapted text is credited; nothing resembles a private assessment. Fix or rewrite anything wrong.

- [ ] **Step 7: Run the tests and checks** — `make lint check test` → PASS.

- [ ] **Step 8: Commit** — `git add -A && git commit -m "Add network-recon"`; NOTICE gains the x-glacier and Decepticon sections if their material was adapted.

### Task B7: `service-enum`

**Files:**
- Create: `skills/service-enum/SKILL.md`, `references/{http.md,tls.md,smb.md,ldap.md,snmp.md,ssh.md,ftp.md,rdp.md,databases.md,rupu.md}`, `scripts/enum.py`, `scripts/_lib/`
- Create: `tests/test_service_enum.py`, fake shims in `tests/fixtures/fake-bin/` for `httpx-toolkit`, `nuclei`, `testssl`, `enum4linux-ng`, `nxc`, `ldapsearch`, `onesixtyone`, `snmpwalk`

**Interfaces:**
- `enum.py <class> --scope <file> --target <host>:<port> [--rate N] --out <dir>` where `<class>` ∈ `http tls smb ldap snmp ssh ftp rdp database`. Runs that class's tools against the one target after the scope check, writes raw output under `<out>/<class>/<host>_<port>/`, prints a JSON summary `{"class","target","tools":[{"tool","argv","exit","output"}],"notes":[]}`. Refuses out-of-scope targets with no tool run. Safe defaults: nuclei with `-severity low,medium,high,critical -exclude-tags dos,fuzz,intrusive -rate-limit <scope.rate>`; netexec/enum4linux-ng with null and guest sessions only (no credentials accepted by the script); snmp with a short public community list (`public`, `private`) only.

- [ ] **Step 1: Tests** `tests/test_service_enum.py`, same shape as `ScopeGate` in B6: for each class, an out-of-scope `--target` is refused with no shim run; an in-scope `http` target runs `httpx-toolkit` then `nuclei` (both in `$FAKE_LOG`) and `nuclei`'s argv contains `-exclude-tags` and `dos`; `smb` argv contains no `-u`/`-p` other than empty/guest; an unknown class exits 2.
- [ ] **Step 2: Run, see them fail.**
- [ ] **Step 3: Brief for Mythos**: the interface above; spec §7's `service-enum` paragraph; per class, what to check and what to stop at (it identifies and verifies exposures — default pages, outdated versions with known CVEs, weak TLS, null/guest SMB shares, anonymous LDAP bind, default SNMP communities — and stops before exploitation); public sources (tool READMEs, testssl.sh docs, the x-glacier per-tool references); `references/rupu.md` (bump `assets.mark` to `tested`; hand verified exposures to `rupu-finding-report`).
- [ ] **Step 4: Run Mythos, `make sync`, review** (same checklist as B6 Step 6, plus: no credential flags reachable; nuclei templates restricted as above).
- [ ] **Step 5: `make lint check test`** → PASS.
- [ ] **Step 6: Commit** — `git commit -am "Add service-enum"` (with `git add -A` first).

### Task B8: `work-units`

**Files:**
- Create: `skills/work-units/SKILL.md`, `references/{sizing.md,rupu.md}`, `scripts/split.py` (in `NO_TARGETS`)
- Create: `tests/test_work_units.py`, `tests/fixtures/{normalized.json,httpx.jsonl,openapi.json,results.sarif,prowler.ocsf.json}`

**Interfaces:**
- `split.py services <file> --by host|class [--max-per-unit N] [-o out.json]` (input: `parse.py` JSON, nmap XML or naabu JSONL — detected by content); `split.py httpx <jsonl> [--by origin]`; `split.py openapi <json|yaml-as-json> [--by resource]`; `split.py sarif <file> --by file|rule`; `split.py prowler <ocsf-json> [--by account-region-service]`.
- Output: a JSON array (so `for_each:` reads it directly); each unit `{"id": str, "title": str, "targets": [obj...], "hints": [str...]}`. `id` is stable for the same input (`<kind>-<key>` lowercased, non-alnum → `-`; `-2`, `-3` suffixes when `--max-per-unit` chunks a unit). Units are sorted by id. Service classes: `web` (http, https, http-alt, http-proxy, any `tunnel: ssl` on an http service), `tls` (any other service with `tunnel: ssl`), `smb` (microsoft-ds, netbios-ssn), `ldap` (ldap, ldaps), `snmp`, `ssh`, `ftp`, `rdp` (ms-wbt-server), `database` (mysql, postgresql, ms-sql-s, oracle-tns, mongodb, redis), `other`. `hints` for a `--by class` unit name the `service-enum` class to run.

- [ ] **Step 1: Fixtures** (TEST-NET addresses, `example.com` names; the SARIF from a made-up two-rule run; the OpenAPI a small invented API with `/users`, `/users/{id}`, `/orders`; the prowler OCSF two findings in invented account `000000000000`).
- [ ] **Step 2: Tests** `tests/test_work_units.py`:

```python
import json
import subprocess
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SPLIT = ROOT / "skills/work-units/scripts/split.py"
FIX = ROOT / "tests/fixtures"


def split(*args):
    r = subprocess.run([sys.executable, "-I", str(SPLIT), *args], capture_output=True, text=True, timeout=30)
    if r.returncode != 0:
        raise AssertionError(r.stderr)
    return json.loads(r.stdout)


class Services(unittest.TestCase):
    def test_by_host(self):
        units = split("services", str(FIX / "nmap-services.xml"), "--by", "host")
        self.assertEqual([u["id"] for u in units], ["host-192-0-2-10", "host-192-0-2-20"])
        self.assertTrue(all({"id", "title", "targets", "hints"} <= u.keys() for u in units))

    def test_by_class(self):
        ids = [u["id"] for u in split("services", str(FIX / "nmap-services.xml"), "--by", "class")]
        for want in ("class-web", "class-smb", "class-ldap", "class-snmp", "class-ssh"):
            self.assertIn(want, ids)

    def test_chunking_is_stable(self):
        a = split("services", str(FIX / "normalized.json"), "--by", "class", "--max-per-unit", "1")
        b = split("services", str(FIX / "normalized.json"), "--by", "class", "--max-per-unit", "1")
        self.assertEqual(a, b)
        self.assertTrue(all(len(u["targets"]) == 1 for u in a))

    def test_output_is_an_array(self):
        self.assertIsInstance(split("services", str(FIX / "naabu.jsonl"), "--by", "host"), list)


class OtherKinds(unittest.TestCase):
    def test_httpx_by_origin(self):
        units = split("httpx", str(FIX / "httpx.jsonl"))
        self.assertTrue(all(u["id"].startswith("origin-") for u in units))

    def test_openapi_by_resource(self):
        ids = [u["id"] for u in split("openapi", str(FIX / "openapi.json"))]
        self.assertEqual(ids, ["resource-orders", "resource-users"])

    def test_sarif_by_rule(self):
        self.assertEqual(len(split("sarif", str(FIX / "results.sarif"), "--by", "rule")), 2)

    def test_prowler(self):
        self.assertGreaterEqual(len(split("prowler", str(FIX / "prowler.ocsf.json"))), 1)

    def test_empty_input_is_an_empty_array(self):
        import tempfile
        with tempfile.NamedTemporaryFile("w", suffix=".json", delete=False) as f:
            f.write("[]")
        self.assertEqual(split("services", f.name, "--by", "host"), [])


if __name__ == "__main__":
    unittest.main()
```

- [ ] **Step 3: Run, see them fail.**
- [ ] **Step 4: Brief for Mythos**: the interface above; spec §7's `work-units` paragraph; `references/sizing.md` — how big a unit should be per profile (network: one host, or one service class across ≤ 16 hosts; web: one origin; api: one resource; code: one file or one rule across ≤ 20 files; cloud: one account × region × service), and why (a unit is one agent's context; too big and it skims, too small and setup dominates); `references/rupu.md` — write the array to a file in the run's working directory (`reports/<name>-units.json`) and have the workflow read it with `for_each: "{{ read_file('reports/<name>-units.json') }}"`; a lead hands each unit to `dispatch` as the task.
- [ ] **Step 5: Run Mythos, review, `make lint check test`** → PASS.
- [ ] **Step 6: Commit** — `git add -A && git commit -m "Add work-units"`.

### Task B9: `rupu-finding-report`

**Files:**
- Create: `skills/rupu-finding-report/SKILL.md`, `references/{fields.md,evidence-blocks.md,example.json}`
- Create: `tests/test_finding_report.py`, `tests/fixtures/finding_report.schema.json`

- [ ] **Step 1:** `rupu findings schema > tests/fixtures/finding_report.schema.json` (rupu ≥ the release with Agent Skills); record the rupu version in the commit message.
- [ ] **Step 2: Test** `tests/test_finding_report.py`: `references/example.json` validates against the schema with `jsonschema.Draft7Validator`; it has `root_cause`, a non-empty `chain`, a PoC block, and every evidence block kind it uses is listed in `references/evidence-blocks.md`; every top-level schema property is mentioned in `references/fields.md` (so the reference cannot fall behind the schema silently).
- [ ] **Step 3: Run, see it fail.**
- [ ] **Step 4: Brief for Mythos**: the schema file; `docs/coverage.md`'s finding-report sections from rupu (copy into the work tree); spec §7's paragraph; the rules — one finding per root cause; pin the locator (host:port for network); classify (CVE when one applies, else CWE); evidence blocks chosen by what proves the claim (`scan_output` for what a scanner saw, `http_exchange` for a request/response, `pcap_ref` for captures, `text`/`table` otherwise); a PoC another tester can rerun; verification status honest (`verified` only when reproduced); severity reasoning. One worked example on an invented host (`203.0.113.20`, an exposed admin interface on an invented product). This skill is rupu-specific by name; its SKILL.md may name `findings.report`.
- [ ] **Step 5: Run Mythos, review** (the example must be obviously invented; no field the schema does not have), `make lint check test` → PASS.
- [ ] **Step 6: Commit** — `git add -A && git commit -m "Add rupu-finding-report"`.

### Task B10: lab verification on kali6 → kali-b

- [ ] **Step 1: Check the lab hosts** (one ssh per host):

```bash
ssh -i ~/.ssh/id_ed25519_kali kali@192.168.64.8 'uname -a; docker --version; docker compose version; ip -br a'
ssh -i ~/.ssh/id_ed25519_kali kali@192.168.64.6 'uname -a; for t in nmap masscan naabu httpx-toolkit nuclei testssl sslscan enum4linux-ng nxc ldapsearch onesixtyone snmpwalk python3; do printf "%s " $t; command -v $t || echo MISSING; done'
```

Confirm the user name and key from `~/.ssh/config` first. If docker is missing on kali-b, `sudo apt-get install -y docker.io docker-compose-plugin` there (matt authorized the lab). Missing tools on kali6 are installed with the skill's own `preflight.py install ... --install` — that exercises it.

- [ ] **Step 2: Bring up the lab on kali-b**: copy `lab/compose.yaml` (kept in the scratchpad, not committed) with services: `nginx:1.27` on 80 and 443 (self-signed cert generated on the host), a Samba container exposing 139/445 with a guest-readable share, an OpenLDAP container on 389 allowing anonymous bind, an snmpd container on 161/udp with community `public`. Before using an image, check it pulls and its documented settings; substitute an equivalent and note it if one does not.
- [ ] **Step 3: Run the chain on kali6** with scope `192.168.64.8/32` and `!192.168.64.6`: `discover.py` → `ports.py --phase top` → `--phase all` → `--phase udp` → `services.py` → `parse.py` → `split.py services --by class` → `enum.py` for each unit's class. Expected: every lab service is found and lands in the right class; `enum.py` reports the guest share, anonymous bind, `public` community and the TLS findings.
- [ ] **Step 4: Refusals before packets.** On kali6, `sudo tcpdump -i any -nn -c 20 host 192.168.64.1 or host 192.168.64.6 and not port 22 &` then run `discover.py` with targets `192.168.64.8 192.168.64.1` and `ports.py` with a hosts file holding `192.168.64.6`. Expected: both refuse, exit non-zero, tcpdump captures nothing for those addresses.
- [ ] **Step 5: Without raw sockets.** Run `discover.py` and `ports.py` as the unprivileged user with no file capabilities: they pick connect-mode fallbacks, say so, and still find the TCP services.
- [ ] **Step 6:** Fix anything the lab shows, re-run `make lint check test`, commit (`Fix <what> found in lab verification`). Tear the lab down (`docker compose down -v`).

### Task B11: create the GitHub repository, release v1.0.0

- [ ] **Step 1: Ask matt in chat** for the go-ahead to create public `Section9Labs/rupu-skills`, showing the README summary and the skill list. Do not proceed without an explicit yes.
- [ ] **Step 2:** `gh repo create Section9Labs/rupu-skills --public --description "Agent Skills for authorized security assessments (agentskills.io SKILL.md format)" --source ~/Code/Oracle/rupu-skills --remote origin` then `git push origin main:main`.
- [ ] **Step 3:** Wait for CI on `main` to pass (`gh run watch`). Fix forward if not.
- [ ] **Step 4:** Protect `main`: require the `check` job and PRs (`gh api -X PUT repos/Section9Labs/rupu-skills/branches/main/protection ...`, or a ruleset; confirm the settings with matt first).
- [ ] **Step 5:** Fill `CHANGELOG.md` (`## v1.0.0`: the five skills) and `COMPATIBILITY.md` (`v1.0.0 | rupu <version CI validated with> |`), commit on a branch, PR, merge after CI. Then tag on `main`: `git tag -a v1.0.0 -m "rupu-skills v1.0.0" && git push origin v1.0.0:refs/tags/v1.0.0`. Record `git rev-parse v1.0.0^{commit}` for Stage C.

---

## Stage C — rupu pins v1.0.0 and the network fleet uses it

Branch: `feat/fleet-skills-v1` from a fresh `origin/main` (after the Stage A PR merged).

### Task C1: bundle the pins

**Files:**
- Modify: every `crates/rupu-coverage/src/profile/builtin/*.toml` `[bundle]`

- [ ] **Step 1:** Add `skills = [...]` to each `[bundle]`, with `tag = "v1.0.0"` and `commit = "<the v1.0.0 commit>"`, source `"Section9Labs/rupu-skills"`, exactly the union of its agents' grants (Task C2's table):

| Profile | skills |
|---|---|
| network, pentest | `network-recon`, `service-enum`, `work-units`, `rupu-finding-report` |
| web, api, redteam | `work-units`, `rupu-finding-report` |
| binary, cloud, code, container, firmware, iac, mobile, sca, secrets, threat-model | `rupu-finding-report` |

- [ ] **Step 2:** `cargo test -p rupu-coverage --lib profile::` → PASS. `cargo test -p rupu-cli --test it fleet_bundle_coverage::` → FAIL in `every_bundled_skill_is_granted_by_some_fleet_agent` (no agent grants yet). That failure is expected and fixed by C2; commit both together after C2.

### Task C2: grant the skills and rewrite the agent bodies

**Files:**
- Modify: every `crates/rupu-cli/templates/fleet/agents/*.md`

- [ ] **Step 1:** Frontmatter `skills:` per agent:

| Agent | `skills:` |
|---|---|
| `recon` | `[network-recon, work-units]` |
| `service-analyst` | `[service-enum, rupu-finding-report]` |
| `exploit-verifier` | `[rupu-finding-report]` |
| `assessment-lead` | `[work-units]` |
| `api-tester`, `appsec-tester`, `binary-analyst`, `cloud-auditor`, `code-auditor`, `container-scanner`, `crawler`, `firmware-analyst`, `iac-reviewer`, `mobile-analyst`, `redteam-operator`, `sca-auditor`, `secret-scanner`, `threat-modeler` | `[rupu-finding-report]` |

- [ ] **Step 2: Rewrite `recon.md`'s body** (frontmatter otherwise unchanged):

```markdown
You perform **network reconnaissance** for an authorized security assessment: discover what is reachable inside the scope, pin every service to a host and port, record each as an asset, and hand the analysts a worklist. You do not exploit.

**Scope — authorized, and strict.** The in-scope hosts and ranges are in your task prompt. Write them to `reports/scope.txt` in the scope-file format your `network-recon` skill describes (deny entries prefixed `!`), and pass that file to every script. The scripts refuse anything outside it; never work around a refusal.

**Method.** Load your `network-recon` skill (`skills.load`) and follow its phases in order: preflight, host discovery, top ports, all TCP ports on live hosts, the curated UDP ports, service detection, parse. Keep its rate defaults unless the task prompt raises them. Keep every raw output file; it is `scan_output` evidence.

**Record coverage.** For every live host call `assets.mark` with `kind: "network:host"`, the `host` coordinate and `depth: "discovered"`. For every open port, `kind: "network:service"`, `host` and `port`, `depth: "enumerated"`, label `"<host>:<port> <service>/<version>"`; then bump the host to `enumerated`.

**Hand off.** Load your `work-units` skill and split the parsed services by service class into `reports/network-units.json`. That file is the analysts' worklist; the workflow fans out over it.

Return a short summary: live hosts, per host the open `port → service/version` list, the units you wrote, and anything that already looks interesting.
```

- [ ] **Step 3: `service-analyst.md`, `exploit-verifier.md`, `assessment-lead.md`.** Replace the inline tool lists with one paragraph each:
  - service-analyst: "Your worklist is one unit from recon (a service class across some hosts). Load `service-enum` and run the class's checks against each target in the unit, with the scope file at `reports/scope.txt`. Verify before reporting. Load `rupu-finding-report` before your first `findings.report` and write each finding in its full profile."
  - exploit-verifier: "Load `rupu-finding-report`; your verdicts update findings in that profile's terms (verification status, PoC, evidence)."
  - assessment-lead: "When discovery returns more targets than one agent should take, load `work-units` and split them; dispatch one unit per agent."
- [ ] **Step 4: Every other analyst agent**: one added paragraph: "Before your first `findings.report`, load your `rupu-finding-report` skill and write every finding in the full profile it describes: evidence blocks, root cause, chain, a reproducible PoC and an honest verification status."
- [ ] **Step 5: Run the lockstep tests** — `cargo test -p rupu-cli --test it fleet_bundle_coverage:: init_manifest_in_sync::` → PASS (all of them, including `shipped_agents_validate`).
- [ ] **Step 6: Commit C1 + C2 together**

```bash
git add crates/rupu-coverage/src/profile/builtin crates/rupu-cli/templates/fleet/agents
git commit -m "feat(fleet): pin rupu-skills v1.0.0 and grant the fleet its skills"
```

### Task C3: `network-assessment` fans out over work units

**Files:**
- Modify: `crates/rupu-cli/templates/fleet/workflows/network-assessment.yaml`

- [ ] **Step 1: Replace the steps:**

```yaml
steps:
  - id: recon
    agent: recon
    actions: []
    prompt: |
      Perform network reconnaissance against the authorized scope:

      {{ inputs.scope }}

      Write the scope to reports/scope.txt, follow your network-recon skill,
      call `assets.mark` for every host and service, and write the service
      work units to reports/network-units.json with your work-units skill.
      Scan ONLY inside the scope above.
  - id: analyze
    agent: service-analyst
    actions: []
    for_each: "{{ read_file('reports/network-units.json') }}"
    max_parallel: 4
    prompt: |
      Test this work unit ({{ loop.index }} of {{ loop.length }}):

      {{ item | tojson }}

      Authorized scope (hard boundary): reports/scope.txt
      ({{ inputs.scope }})

      Run your service-enum skill against every target in the unit, verify
      before reporting, and `findings.report` each verified exposure in the
      full profile. No destructive actions. Return what you tested and recorded.
  - id: verify
    agent: exploit-verifier
    actions: []
    prompt: |
      Independently confirm the exposures the analysts reported:

      {{ steps.analyze.results | tojson }}

      Authorized scope (hard boundary): {{ inputs.scope }}

      Reproduce each claimed exposure, capture proof, and record a verdict
      (confirmed / false positive / needs-preconditions). Non-destructive only.
      Return what you confirmed and refuted.
```

Update the workflow's `description` to mention the fan-out. Check `{{ steps.analyze.results | tojson }}` against `docs/workflow-format.md` ("Fan-out outputs") and an existing fan-out workflow before relying on it; if `results` renders differently, use what that doc shows.

- [ ] **Step 2:** `cargo test -p rupu-cli --test it fleet_bundle_coverage::every_fleet_workflow_parses` → PASS. `cargo test -p rupu-orchestrator` tests that lint workflow templates, if any reference fleet files (`grep -rn "network-assessment" crates/*/tests`) → PASS.
- [ ] **Step 3: Commit** — `git add crates/rupu-cli/templates/fleet/workflows && git commit -m "feat(fleet): network-assessment fans out over recon's work units"`.

### Task C4: end-to-end check, docs, PR

- [ ] **Step 1: Networked pin check with this branch's binary**: `cargo build -p rupu-cli && scripts/check-skill-pins.sh target/debug/rupu` → `check-skill-pins: ok`, with `CREATED skills/...` lines for the four bundled skills.
- [ ] **Step 2: `--skills-from` for real**: `git clone https://github.com/Section9Labs/rupu-skills /tmp/rs && git -C /tmp/rs checkout <pin>`, then `H=$(mktemp -d); RUPU_HOME=$H target/debug/rupu fleet install --skills-from /tmp/rs` → four CREATED lines; `RUPU_HOME=$H target/debug/rupu skill update --all` → every fleet skill up to date; `rm -rf $H /tmp/rs`.
- [ ] **Step 3: A live network-assessment run in the lab** (matt's go-ahead covers kali-b): on kali6 with this branch's binary installed to a throwaway `RUPU_HOME` and `rupu fleet install`, run `network-assessment` with input `scope=192.168.64.8/32` (check `rupu workflow run --help` for the input flag) with the lab up. Expected: recon writes `reports/scope.txt` and `reports/network-units.json`; `analyze` fans out one unit per service class; findings appear in `rupu findings list` in the full profile. Note anything the agents misread in the skills and fix the skill (a rupu-skills `v1.0.1`, re-pin) or the agent body.
- [ ] **Step 4: Docs**: `docs/engagement-profiles.md` lists each built-in's bundled skills; `docs/skills.md#fleet-skills` names the current pin; CLAUDE.md `rupu-cli` entry notes the fleet's skill grants and that `network-assessment` reads `reports/network-units.json`.
- [ ] **Step 5: Verify locally** — the Stage A test commands, plus `cargo test -p rupu-cli --test it fleet_bundle_coverage:: init_manifest_in_sync::` → PASS.
- [ ] **Step 6: PR** — rebase on `origin/main`, push with an explicit refspec, `gh pr create` with the lab-run summary. Do not merge without matt.

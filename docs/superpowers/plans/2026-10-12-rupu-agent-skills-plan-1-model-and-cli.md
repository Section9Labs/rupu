# Agent Skills Plan 1: skill model and `rupu skill` CLI

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A new leaf crate `rupu-skills` that parses, validates, discovers, fingerprints, grants, renders, installs and updates agentskills.io skills, plus `rupu skill list|show|validate|install|update`.

**Architecture:** `rupu-skills` has no rupu dependencies. It owns the `Skill` model and every pure rule the spec states (name rules, frontmatter, the grant grammar, the stamp and content hash, the prompt renderers). Plans 2 to 4 call it from `rupu-tools`, `rupu-agent`, `rupu-runtime` and the CP. Installs accept the sources people copy (a path, `owner/repo`, a GitHub tree URL, any https git URL), search the source for skills, and record where each came from so `update` can re-fetch it. A `git` cargo feature adds the libgit2 fetcher, so only `rupu-cli` links `git2` through it. The CLI is a thin `cmd/skill.rs` over the crate. The layer directories come from one new `rupu_workspace::project_layer_dir`, so the R5 rule is the same one config loading uses.

**Tech Stack:** Rust 2021, `serde`/`serde_yaml`, `sha2` + `hex`, `walkdir`, `thiserror`, `git2` (feature `git`), `clap`, `assert_cmd`.

**Spec:** `docs/superpowers/specs/2026-10-12-rupu-agent-skills-design.md` (§1 model, §2.1 grant grammar, §3.1 to §3.3 renderers, §3.5 tool map, §7 CLI, sources, search and `update`). Read it before starting.

## Global Constraints

- Versions are pinned in the root `Cargo.toml` only. Crate `Cargo.toml` files use `{ workspace = true }`.
- `[lints] workspace = true` in the new crate. `unsafe_code` is forbidden and `clippy::all` is denied.
- `rupu-skills` depends on no `rupu-*` crate.
- Library errors use `thiserror`. Only `rupu-cli` uses `anyhow`.
- Integration tests: ONE binary per crate, `crates/<c>/tests/it/main.rs` listing modules. Never add a top-level `tests/*.rs`.
- `rupu-cli` is thin: `cmd/skill.rs` parses args, calls `rupu-skills`, prints.
- Name rule, verbatim from the spec: 1 to 64 chars, `a-z`, `0-9`, `-`, no leading, trailing or doubled `-`, equal to the directory name.
- `description` is 1 to 1024 chars. `compatibility`, when present, is 1 to 500 chars. Characters are counted as `char`s.
- `*` covers the global layer only. `project:*` and exact names reach project skills. A bare name in both layers is an error.
- The stamp and the content hash skip `.git` and `.DS_Store` entries at any depth.
- Fixture skills are invented from scratch. Never adapt real assessment material (public repo).
- Format only the files you touch (`rustfmt <file>`). Never run package-wide `cargo fmt`: `main` has rustfmt drift.
- Never use bare `git stash`.

## Review Focus

1. **A Windows-authored `SKILL.md`** (UTF-8 BOM, CRLF line endings) parses the same as a Unix one. Pinned in Task 2.
2. **A skill directory that is a git checkout or was opened in Finder** (`.git/`, `.DS_Store`) gets a stamp that does not move when git fetches or Finder writes. Pinned in Task 3.
3. **`install --force` with a broken replacement** leaves the existing skill untouched. Pinned in Task 7.
4. **`allowed-tools: Bash(git status:*) Read`**, with a space inside the parentheses, maps to `bash` and `read_file`, not to three garbage entries. Pinned in Task 6.
5. **A GitHub URL copied from the browser** (`https://github.com/o/r/tree/main/skills/pdf`) installs that one skill rather than failing to clone. Pinned in Task 8.
6. **A `metadata:` value written as a YAML number** (`version: 1.0`) is accepted as the string `"1.0"`. Published skills do this often, and rejecting them breaks format compatibility. Pinned in Task 2.

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/rupu-skills/Cargo.toml` | crate manifest, `git` feature |
| `crates/rupu-skills/src/lib.rs` | module list and re-exports |
| `crates/rupu-skills/src/name.rs` | `SkillName`, `NameError` |
| `crates/rupu-skills/src/frontmatter.rs` | `SKILL.md` split + frontmatter parse, `Frontmatter`, `FrontmatterError` |
| `crates/rupu-skills/src/tree.rs` | tree walk, `TreeEntry`, `Stamp`, `ContentHash`, `TreeError` |
| `crates/rupu-skills/src/skill.rs` | `Layer`, `Skill`, `load_skill`, `SkillError` |
| `crates/rupu-skills/src/catalog.rs` | `SkillCatalog`, `CatalogEntry`, `LayerDirs`, `DiscoverError` |
| `crates/rupu-skills/src/grant.rs` | `SkillGrant` grammar, `resolve_grants`, `ResolveError`, edit distance |
| `crates/rupu-skills/src/render.rs` | skills section (preload + index), `skills.load` result text |
| `crates/rupu-skills/src/tools_map.rs` | `allowed-tools` → rupu core tool names |
| `crates/rupu-skills/src/install.rs` | `find_skills`, `choose`, `install_selected`, `install_one`, `install_from`, `list_source`, `InstallError` |
| `crates/rupu-skills/src/source.rs` | `Source`, `GitSource`, `parse_source`, `Fetch`, `FetchedTree`, `SourceError` |
| `crates/rupu-skills/src/git.rs` | `GitFetcher` (feature `git`) |
| `crates/rupu-skills/src/record.rs` | `SourceRecord`, `.sources/<name>.json` read/write |
| `crates/rupu-skills/src/update.rs` | `update_skills`, `UpdateOutcome` |
| `crates/rupu-skills/tests/it/*.rs` | one integration binary |
| `crates/rupu-workspace/src/config_paths.rs` | `project_layer_dir` (R5), reused by `project_config_path` |
| `crates/rupu-cli/src/cmd/skill.rs` | `rupu skill` subcommands |
| `crates/rupu-cli/tests/it/cli_skill.rs` | CLI end to end |
| `docs/skills.md` | operator doc: format, layers, CLI |

---

### Task 1: Crate scaffold and `SkillName`

**Files:**
- Modify: `Cargo.toml` (root: `members`, `[workspace.dependencies]`)
- Create: `crates/rupu-skills/Cargo.toml`, `crates/rupu-skills/src/lib.rs`, `crates/rupu-skills/src/name.rs`
- Test: `crates/rupu-skills/tests/it/main.rs`, `crates/rupu-skills/tests/it/name.rs`

**Interfaces:**
- Produces: `rupu_skills::{SkillName, NameError, MAX_NAME_LEN}`. `SkillName: FromStr<Err = NameError> + Display + Ord + Hash + Clone + Serialize`, with `as_str(&self) -> &str`.

- [ ] **Step 1: Register the crate**

Root `Cargo.toml`: add `"crates/rupu-skills",` to `members` after `"crates/rupu-codename",`, and next to `rupu-codename = { path = "crates/rupu-codename" }` add:

```toml
rupu-skills = { path = "crates/rupu-skills" }
```

`crates/rupu-skills/Cargo.toml`:

```toml
[package]
name = "rupu-skills"
version.workspace = true
edition.workspace = true
license.workspace = true
repository.workspace = true
rust-version.workspace = true

[lints]
workspace = true

[features]
git = ["dep:git2"]

[dependencies]
serde = { workspace = true, features = ["derive"] }
serde_json = { workspace = true }
serde_yaml = { workspace = true }
sha2 = { workspace = true }
hex = { workspace = true }
walkdir = { workspace = true }
thiserror = { workspace = true }
tempfile = { workspace = true }
git2 = { workspace = true, optional = true }

[dev-dependencies]
git2 = { workspace = true }
```

`crates/rupu-skills/src/lib.rs`:

```rust
//! Agent Skills (agentskills.io `SKILL.md` directories): the model, its
//! validation, discovery over rupu's global and project layers, version
//! fingerprints, the grant grammar and the prompt renderers. Spec:
//! `docs/superpowers/specs/2026-10-12-rupu-agent-skills-design.md`.

pub mod name;

pub use name::{NameError, SkillName, MAX_NAME_LEN};
```

`crates/rupu-skills/tests/it/main.rs`:

```rust
mod name;
```

- [ ] **Step 2: Write the failing test** (`crates/rupu-skills/tests/it/name.rs`)

```rust
use rupu_skills::{NameError, SkillName};

#[test]
fn valid_names_parse() {
    for n in ["pdf-processing", "a", "code-review", "x1-2y", &"a".repeat(64)] {
        let parsed: SkillName = n.parse().unwrap();
        assert_eq!(parsed.as_str(), n);
        assert_eq!(parsed.to_string(), n);
    }
}

#[test]
fn invalid_names_say_why() {
    assert_eq!("".parse::<SkillName>(), Err(NameError::Empty));
    assert_eq!("a".repeat(65).parse::<SkillName>(), Err(NameError::TooLong(65)));
    assert_eq!("PDF-Processing".parse::<SkillName>(), Err(NameError::BadChar('P')));
    assert_eq!("pdf_x".parse::<SkillName>(), Err(NameError::BadChar('_')));
    assert_eq!("pdf.x".parse::<SkillName>(), Err(NameError::BadChar('.')));
    assert_eq!("-pdf".parse::<SkillName>(), Err(NameError::EdgeHyphen));
    assert_eq!("pdf-".parse::<SkillName>(), Err(NameError::EdgeHyphen));
    assert_eq!("pdf--processing".parse::<SkillName>(), Err(NameError::DoubleHyphen));
}

#[test]
fn serializes_as_a_plain_string() {
    let n: SkillName = "code-review".parse().unwrap();
    assert_eq!(serde_json::to_string(&n).unwrap(), "\"code-review\"");
}
```

- [ ] **Step 2b: Run it and watch it fail**

Run: `cargo test -p rupu-skills --test it name::`
Expected: compile error, `name` module not found.

- [ ] **Step 3: Implement** (`crates/rupu-skills/src/name.rs`)

```rust
//! A skill's name, validated per the agentskills.io rules.

use serde::Serialize;
use std::fmt;
use std::str::FromStr;

pub const MAX_NAME_LEN: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct SkillName(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NameError {
    #[error("skill name is empty")]
    Empty,
    #[error("skill name is {0} characters; the limit is 64")]
    TooLong(usize),
    #[error("skill name may only contain a-z, 0-9 and '-'; found {0:?}")]
    BadChar(char),
    #[error("skill name must not start or end with '-'")]
    EdgeHyphen,
    #[error("skill name must not contain '--'")]
    DoubleHyphen,
}

impl SkillName {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for SkillName {
    type Err = NameError;

    fn from_str(s: &str) -> Result<Self, NameError> {
        if s.is_empty() {
            return Err(NameError::Empty);
        }
        let len = s.chars().count();
        if len > MAX_NAME_LEN {
            return Err(NameError::TooLong(len));
        }
        if let Some(c) = s.chars().find(|c| !matches!(c, 'a'..='z' | '0'..='9' | '-')) {
            return Err(NameError::BadChar(c));
        }
        if s.starts_with('-') || s.ends_with('-') {
            return Err(NameError::EdgeHyphen);
        }
        if s.contains("--") {
            return Err(NameError::DoubleHyphen);
        }
        Ok(Self(s.to_owned()))
    }
}

impl fmt::Display for SkillName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
```

- [ ] **Step 4: Run it and watch it pass**

Run: `cargo test -p rupu-skills --test it name::`
Expected: 3 passed.

- [ ] **Step 5: Commit**

```bash
rustfmt crates/rupu-skills/src/*.rs crates/rupu-skills/tests/it/*.rs
git add Cargo.toml Cargo.lock crates/rupu-skills
git commit -m "feat(skills): add rupu-skills crate with SkillName"
```

---

### Task 2: `SKILL.md` frontmatter

**Files:**
- Create: `crates/rupu-skills/src/frontmatter.rs`
- Modify: `crates/rupu-skills/src/lib.rs`, `crates/rupu-skills/tests/it/main.rs`
- Test: `crates/rupu-skills/tests/it/frontmatter.rs`

**Interfaces:**
- Produces: `rupu_skills::{parse_skill_md, Frontmatter, FrontmatterError, MAX_DESCRIPTION_LEN, MAX_COMPATIBILITY_LEN}`.
  - `parse_skill_md(text: &str) -> Result<(Frontmatter, String), FrontmatterError>` returns the frontmatter and the body. The body has its leading blank lines and trailing whitespace trimmed.
  - `Frontmatter { name: String, description: String, license: Option<String>, compatibility: Option<String>, metadata: BTreeMap<String, String>, allowed_tools: Vec<String>, disable_model_invocation: bool }`, deriving `Debug, Clone, PartialEq, Eq, Default, Serialize`. `name` is the raw string; Task 4 validates it against the directory.

- [ ] **Step 1: Write the failing test** (`crates/rupu-skills/tests/it/frontmatter.rs`; add `mod frontmatter;` to `main.rs`)

```rust
use rupu_skills::{parse_skill_md, FrontmatterError};

#[test]
fn minimal_skill() {
    let (fm, body) = parse_skill_md(
        "---\nname: pdf-processing\ndescription: Extract PDF text. Use when handling PDFs.\n---\n\n# PDF\n\nStep one.\n",
    )
    .unwrap();
    assert_eq!(fm.name, "pdf-processing");
    assert_eq!(fm.description, "Extract PDF text. Use when handling PDFs.");
    assert_eq!(fm.license, None);
    assert!(fm.allowed_tools.is_empty());
    assert!(!fm.disable_model_invocation);
    assert_eq!(body, "# PDF\n\nStep one.");
}

#[test]
fn optional_fields_and_unknown_keys() {
    let text = "---\n\
name: report-style\n\
description: House style for reports.\n\
license: Apache-2.0\n\
compatibility: Requires Python 3.12+ and uv\n\
metadata:\n  author: example-org\n  version: 1.0\n  beta: true\n\
allowed-tools: Bash(git status:*) Read, Grep\n\
disable-model-invocation: true\n\
argument-hint: \"[file]\"\n\
---\nBody\n";
    let (fm, body) = parse_skill_md(text).unwrap();
    assert_eq!(fm.license.as_deref(), Some("Apache-2.0"));
    assert_eq!(fm.compatibility.as_deref(), Some("Requires Python 3.12+ and uv"));
    assert_eq!(fm.metadata.get("author").map(String::as_str), Some("example-org"));
    assert_eq!(fm.metadata.get("version").map(String::as_str), Some("1.0"));
    assert_eq!(fm.metadata.get("beta").map(String::as_str), Some("true"));
    assert_eq!(fm.allowed_tools, vec!["Bash(git status:*)", "Read", "Grep"]);
    assert!(fm.disable_model_invocation);
    assert_eq!(body, "Body");
}

#[test]
fn allowed_tools_as_a_list() {
    let (fm, _) = parse_skill_md(
        "---\nname: a\ndescription: d\nallowed-tools:\n  - Read\n  - Bash(jq:*)\n---\n",
    )
    .unwrap();
    assert_eq!(fm.allowed_tools, vec!["Read", "Bash(jq:*)"]);
}

#[test]
fn bom_and_crlf() {
    let text = "\u{feff}---\r\nname: a\r\ndescription: d\r\n---\r\nLine one\r\nLine two\r\n";
    let (fm, body) = parse_skill_md(text).unwrap();
    assert_eq!(fm.name, "a");
    assert_eq!(body, "Line one\r\nLine two");
}

#[test]
fn empty_body_is_fine() {
    let (_, body) = parse_skill_md("---\nname: a\ndescription: d\n---\n").unwrap();
    assert_eq!(body, "");
}

#[test]
fn errors() {
    assert_eq!(parse_skill_md("# no frontmatter\n"), Err(FrontmatterError::MissingOpen));
    assert_eq!(parse_skill_md("---\nname: a\n"), Err(FrontmatterError::MissingClose));
    assert_eq!(parse_skill_md("---\ndescription: d\n---\n"), Err(FrontmatterError::MissingName));
    assert_eq!(parse_skill_md("---\nname: a\n---\n"), Err(FrontmatterError::MissingDescription));
    assert_eq!(
        parse_skill_md("---\nname: a\ndescription: \"\"\n---\n"),
        Err(FrontmatterError::MissingDescription)
    );
    let long = format!("---\nname: a\ndescription: {}\n---\n", "x".repeat(1025));
    assert_eq!(parse_skill_md(&long), Err(FrontmatterError::DescriptionTooLong(1025)));
    let compat = format!("---\nname: a\ndescription: d\ncompatibility: {}\n---\n", "x".repeat(501));
    assert_eq!(parse_skill_md(&compat), Err(FrontmatterError::CompatibilityLength(501)));
    assert_eq!(
        parse_skill_md("---\nname: a\ndescription: d\nmetadata:\n  nested: [1, 2]\n---\n"),
        Err(FrontmatterError::Metadata("nested".into()))
    );
    assert!(matches!(
        parse_skill_md("---\nname: [unclosed\n---\n"),
        Err(FrontmatterError::Yaml(_))
    ));
}
```

- [ ] **Step 2: Run it and watch it fail**

Run: `cargo test -p rupu-skills --test it frontmatter::`
Expected: compile error, `parse_skill_md` not found.

- [ ] **Step 3: Implement** (`crates/rupu-skills/src/frontmatter.rs`)

```rust
//! `SKILL.md`: YAML frontmatter between `---` lines, then the instructions.
//! Unknown keys are accepted: harnesses add their own, and rejecting them
//! would break format compatibility.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const MAX_DESCRIPTION_LEN: usize = 1024;
pub const MAX_COMPATIBILITY_LEN: usize = 500;

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct Frontmatter {
    pub name: String,
    pub description: String,
    pub license: Option<String>,
    pub compatibility: Option<String>,
    pub metadata: BTreeMap<String, String>,
    pub allowed_tools: Vec<String>,
    pub disable_model_invocation: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FrontmatterError {
    #[error("SKILL.md must start with a `---` frontmatter line")]
    MissingOpen,
    #[error("SKILL.md frontmatter has no closing `---` line")]
    MissingClose,
    #[error("frontmatter YAML: {0}")]
    Yaml(String),
    #[error("frontmatter has no `name`")]
    MissingName,
    #[error("frontmatter has no `description` (or it is empty)")]
    MissingDescription,
    #[error("`description` is {0} characters; the limit is 1024")]
    DescriptionTooLong(usize),
    #[error("`compatibility` is {0} characters; it must be 1 to 500")]
    CompatibilityLength(usize),
    #[error("`metadata.{0}` must be a string, number or boolean")]
    Metadata(String),
}

#[derive(Deserialize)]
struct Raw {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    license: Option<String>,
    #[serde(default)]
    compatibility: Option<String>,
    #[serde(default)]
    metadata: BTreeMap<String, serde_yaml::Value>,
    #[serde(default, rename = "allowed-tools")]
    allowed_tools: Option<AllowedTools>,
    #[serde(default, rename = "disable-model-invocation")]
    disable_model_invocation: bool,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum AllowedTools {
    One(String),
    Many(Vec<String>),
}

pub fn parse_skill_md(text: &str) -> Result<(Frontmatter, String), FrontmatterError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines = text.split_inclusive('\n');
    match lines.next() {
        Some(first) if first.trim_end() == "---" => {}
        _ => return Err(FrontmatterError::MissingOpen),
    }
    let mut yaml = String::new();
    let mut closed = false;
    for line in lines.by_ref() {
        if line.trim_end() == "---" {
            closed = true;
            break;
        }
        yaml.push_str(line);
    }
    if !closed {
        return Err(FrontmatterError::MissingClose);
    }
    let body: String = lines.collect();
    let body = body.trim_start_matches(['\r', '\n']).trim_end().to_owned();

    if yaml.trim().is_empty() {
        return Err(FrontmatterError::MissingName);
    }
    let raw: Raw = serde_yaml::from_str(&yaml).map_err(|e| FrontmatterError::Yaml(e.to_string()))?;

    let name = raw.name.filter(|n| !n.is_empty()).ok_or(FrontmatterError::MissingName)?;
    let description = raw
        .description
        .filter(|d| !d.trim().is_empty())
        .ok_or(FrontmatterError::MissingDescription)?;
    let len = description.chars().count();
    if len > MAX_DESCRIPTION_LEN {
        return Err(FrontmatterError::DescriptionTooLong(len));
    }
    if let Some(c) = &raw.compatibility {
        let len = c.chars().count();
        if len == 0 || len > MAX_COMPATIBILITY_LEN {
            return Err(FrontmatterError::CompatibilityLength(len));
        }
    }
    let mut metadata = BTreeMap::new();
    for (k, v) in raw.metadata {
        let s = match v {
            serde_yaml::Value::String(s) => s,
            serde_yaml::Value::Number(n) => n.to_string(),
            serde_yaml::Value::Bool(b) => b.to_string(),
            _ => return Err(FrontmatterError::Metadata(k)),
        };
        metadata.insert(k, s);
    }
    let allowed_tools = match raw.allowed_tools {
        None => Vec::new(),
        Some(AllowedTools::One(s)) => split_tool_list(&s),
        Some(AllowedTools::Many(v)) => v.into_iter().map(|t| t.trim().to_owned()).filter(|t| !t.is_empty()).collect(),
    };
    Ok((
        Frontmatter {
            name,
            description,
            license: raw.license,
            compatibility: raw.compatibility,
            metadata,
            allowed_tools,
            disable_model_invocation: raw.disable_model_invocation,
        },
        body,
    ))
}

/// Split on whitespace and commas outside parentheses, so
/// `Bash(git status:*) Read, Grep` is three entries, not five.
fn split_tool_list(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut depth = 0usize;
    for c in s.chars() {
        match c {
            '(' => {
                depth += 1;
                cur.push(c);
            }
            ')' => {
                depth = depth.saturating_sub(1);
                cur.push(c);
            }
            c if depth == 0 && (c.is_whitespace() || c == ',') => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}
```

`lib.rs`: add `pub mod frontmatter;` and `pub use frontmatter::{parse_skill_md, Frontmatter, FrontmatterError, MAX_COMPATIBILITY_LEN, MAX_DESCRIPTION_LEN};`.

`serde_yaml` reads `version: 1.0` as a float, and its `Number` displays floats through `ryu`, which writes `1.0`. A value like `1.10` comes back as `1.1`. Authors who care quote the value, as the agentskills.io examples do.

- [ ] **Step 4: Run it and watch it pass**

Run: `cargo test -p rupu-skills --test it frontmatter::`
Expected: 6 passed.

- [ ] **Step 5: Commit**

```bash
rustfmt crates/rupu-skills/src/frontmatter.rs crates/rupu-skills/src/lib.rs crates/rupu-skills/tests/it/frontmatter.rs crates/rupu-skills/tests/it/main.rs
git add crates/rupu-skills
git commit -m "feat(skills): parse SKILL.md frontmatter"
```

---

### Task 3: Tree walk, stamp and content hash

**Files:**
- Create: `crates/rupu-skills/src/tree.rs`
- Modify: `crates/rupu-skills/src/lib.rs`, `crates/rupu-skills/tests/it/main.rs`
- Test: `crates/rupu-skills/tests/it/tree.rs`

**Interfaces:**
- Produces:
  - `walk_tree(root: &Path) -> Result<Vec<TreeEntry>, TreeError>`: entries in path order, files and symlinks only, `.git` and `.DS_Store` skipped at any depth. Each symlink must resolve inside `root`.
  - `TreeEntry { rel: String /* '/'-separated */, kind: EntryKind }`; `EntryKind::{File { len: u64, mtime_ns: u128, mode: u32 }, Symlink { target: String }}`.
  - `stamp(entries: &[TreeEntry]) -> Stamp`.
  - `content_hash(root: &Path, entries: &[TreeEntry]) -> Result<ContentHash, TreeError>`.
  - `Stamp` and `ContentHash`: `Copy + Eq + Hash + Debug`, `hex(&self) -> String`, `short(&self) -> String` (first 12 hex chars), `Serialize` as the hex string.
  - `TreeError::{Io { path: PathBuf, source: std::io::Error }, NotUtf8(PathBuf), BadSymlink { rel: String, reason: String }}`.

- [ ] **Step 1: Write the failing test** (`crates/rupu-skills/tests/it/tree.rs`; add `mod tree;`)

```rust
use rupu_skills::tree::{content_hash, stamp, walk_tree, EntryKind, TreeError};
use std::fs;
use std::path::Path;

fn write(p: &Path, s: &str) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, s).unwrap();
}

fn skill(dir: &Path) {
    write(&dir.join("SKILL.md"), "---\nname: s\ndescription: d\n---\n");
    write(&dir.join("scripts/run.sh"), "echo hi\n");
    write(&dir.join("references/REF.md"), "ref\n");
}

#[test]
fn walk_is_sorted_relative_and_skips_noise() {
    let t = tempfile::tempdir().unwrap();
    skill(t.path());
    write(&t.path().join(".git/HEAD"), "ref: refs/heads/main\n");
    write(&t.path().join(".DS_Store"), "x");
    write(&t.path().join("references/.DS_Store"), "x");
    fs::create_dir_all(t.path().join("empty")).unwrap();
    let rels: Vec<String> = walk_tree(t.path()).unwrap().into_iter().map(|e| e.rel).collect();
    assert_eq!(rels, vec!["SKILL.md", "references/REF.md", "scripts/run.sh"]);
}

#[test]
fn stamp_ignores_noise_and_tracks_edits() {
    let t = tempfile::tempdir().unwrap();
    skill(t.path());
    let a = stamp(&walk_tree(t.path()).unwrap());
    write(&t.path().join(".git/FETCH_HEAD"), "abc\n");
    write(&t.path().join(".DS_Store"), "y");
    assert_eq!(a, stamp(&walk_tree(t.path()).unwrap()), "noise moved the stamp");

    write(&t.path().join("references/NEW.md"), "new\n");
    let b = stamp(&walk_tree(t.path()).unwrap());
    assert_ne!(a, b, "an added file must move the stamp");
}

#[cfg(unix)]
#[test]
fn chmod_moves_both_fingerprints() {
    use std::os::unix::fs::PermissionsExt;
    let t = tempfile::tempdir().unwrap();
    skill(t.path());
    let e1 = walk_tree(t.path()).unwrap();
    let (s1, h1) = (stamp(&e1), content_hash(t.path(), &e1).unwrap());
    fs::set_permissions(t.path().join("scripts/run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
    let e2 = walk_tree(t.path()).unwrap();
    assert_ne!(s1, stamp(&e2));
    assert_ne!(h1, content_hash(t.path(), &e2).unwrap());
}

#[test]
fn content_hash_is_stable_and_content_sensitive() {
    let t = tempfile::tempdir().unwrap();
    skill(t.path());
    let e = walk_tree(t.path()).unwrap();
    let h = content_hash(t.path(), &e).unwrap();
    assert_eq!(h, content_hash(t.path(), &e).unwrap());
    assert_eq!(h.hex().len(), 64);
    assert_eq!(h.short(), h.hex()[..12]);

    let u = tempfile::tempdir().unwrap();
    skill(u.path());
    write(&u.path().join("scripts/run.sh"), "echo bye\n");
    let eu = walk_tree(u.path()).unwrap();
    assert_ne!(h, content_hash(u.path(), &eu).unwrap());
}

#[cfg(unix)]
#[test]
fn symlinks_inside_are_kept_outside_are_refused() {
    let t = tempfile::tempdir().unwrap();
    skill(t.path());
    std::os::unix::fs::symlink("references/REF.md", t.path().join("REF-LINK.md")).unwrap();
    let e = walk_tree(t.path()).unwrap();
    let link = e.iter().find(|x| x.rel == "REF-LINK.md").unwrap();
    assert!(matches!(&link.kind, EntryKind::Symlink { target } if target == "references/REF.md"));

    let outside = tempfile::tempdir().unwrap();
    write(&outside.path().join("secret"), "s");
    std::os::unix::fs::symlink(outside.path().join("secret"), t.path().join("leak")).unwrap();
    assert!(matches!(walk_tree(t.path()), Err(TreeError::BadSymlink { rel, .. }) if rel == "leak"));
}

#[cfg(unix)]
#[test]
fn dangling_symlink_is_refused() {
    let t = tempfile::tempdir().unwrap();
    skill(t.path());
    std::os::unix::fs::symlink("nowhere", t.path().join("dangle")).unwrap();
    assert!(matches!(walk_tree(t.path()), Err(TreeError::BadSymlink { .. })));
}
```

- [ ] **Step 2: Run it and watch it fail**

Run: `cargo test -p rupu-skills --test it tree::`
Expected: compile error, module `tree` not found.

- [ ] **Step 3: Implement** (`crates/rupu-skills/src/tree.rs`)

```rust
//! A skill directory's files and its two fingerprints (spec §1.3). The
//! stamp costs one `stat` per file and is what an index records. The
//! content hash reads every byte and is computed only when a skill loads.

use serde::{Serialize, Serializer};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// Never part of a skill: VCS metadata and Finder litter.
pub const IGNORED_NAMES: &[&str] = &[".git", ".DS_Store"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeEntry {
    pub rel: String,
    pub kind: EntryKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryKind {
    File { len: u64, mtime_ns: u128, mode: u32 },
    Symlink { target: String },
}

#[derive(Debug, thiserror::Error)]
pub enum TreeError {
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
    #[error("{0}: path is not UTF-8")]
    NotUtf8(PathBuf),
    #[error("symlink {rel}: {reason}")]
    BadSymlink { rel: String, reason: String },
}

macro_rules! fingerprint {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub struct $name(pub [u8; 32]);

        impl $name {
            pub fn hex(&self) -> String {
                hex::encode(self.0)
            }
            pub fn short(&self) -> String {
                self.hex()[..12].to_owned()
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(&self.hex())
            }
        }
    };
}

fingerprint!(Stamp);
fingerprint!(ContentHash);

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> TreeError + '_ {
    move |source| TreeError::Io { path: path.to_owned(), source }
}

pub fn walk_tree(root: &Path) -> Result<Vec<TreeEntry>, TreeError> {
    let root = root.canonicalize().map_err(io(root))?;
    let walker = walkdir::WalkDir::new(&root)
        .min_depth(1)
        .follow_links(false)
        .sort_by_file_name()
        .into_iter()
        .filter_entry(|e| !e.file_name().to_str().is_some_and(|n| IGNORED_NAMES.contains(&n)));
    let mut out = Vec::new();
    for entry in walker {
        let entry = entry.map_err(|e| {
            let path = e.path().map(Path::to_owned).unwrap_or_else(|| root.clone());
            TreeError::Io { path, source: e.into() }
        })?;
        let path = entry.path();
        let rel = path
            .strip_prefix(&root)
            .ok()
            .and_then(Path::to_str)
            .ok_or_else(|| TreeError::NotUtf8(path.to_owned()))?
            .replace('\\', "/");
        let ft = entry.file_type();
        if ft.is_dir() {
            continue;
        }
        if ft.is_symlink() {
            let target = std::fs::read_link(path).map_err(io(path))?;
            let target = target.to_str().ok_or_else(|| TreeError::NotUtf8(path.to_owned()))?.to_owned();
            let resolved = path.canonicalize().map_err(|e| TreeError::BadSymlink {
                rel: rel.clone(),
                reason: format!("does not resolve ({e})"),
            })?;
            if !resolved.starts_with(&root) {
                return Err(TreeError::BadSymlink {
                    rel,
                    reason: format!("points outside the skill ({})", resolved.display()),
                });
            }
            out.push(TreeEntry { rel, kind: EntryKind::Symlink { target } });
            continue;
        }
        let md = entry.metadata().map_err(|e| TreeError::Io { path: path.to_owned(), source: e.into() })?;
        let mtime_ns = md
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_nanos());
        out.push(TreeEntry { rel, kind: EntryKind::File { len: md.len(), mtime_ns, mode: mode_of(&md) } });
    }
    Ok(out)
}

#[cfg(unix)]
fn mode_of(md: &std::fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    md.permissions().mode() & 0o777
}

#[cfg(not(unix))]
fn mode_of(_: &std::fs::Metadata) -> u32 {
    0
}

pub fn stamp(entries: &[TreeEntry]) -> Stamp {
    let mut h = Sha256::new();
    for e in entries {
        h.update(e.rel.as_bytes());
        h.update([0]);
        match &e.kind {
            EntryKind::File { len, mtime_ns, mode } => {
                h.update(format!("f\0{len}\0{mtime_ns}\0{mode:o}\n").as_bytes());
            }
            EntryKind::Symlink { target } => {
                h.update(b"l\0");
                h.update(target.as_bytes());
                h.update(b"\n");
            }
        }
    }
    Stamp(h.finalize().into())
}

pub fn content_hash(root: &Path, entries: &[TreeEntry]) -> Result<ContentHash, TreeError> {
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    for e in entries {
        h.update(e.rel.as_bytes());
        h.update([0]);
        match &e.kind {
            EntryKind::File { mode, .. } => {
                let path = root.join(&e.rel);
                let mut f = std::fs::File::open(&path).map_err(io(&path))?;
                let len = f.metadata().map_err(io(&path))?.len();
                h.update(format!("f\0{mode:o}\0{len}\0").as_bytes());
                loop {
                    let n = f.read(&mut buf).map_err(io(&path))?;
                    if n == 0 {
                        break;
                    }
                    h.update(&buf[..n]);
                }
            }
            EntryKind::Symlink { target } => {
                h.update(b"l\0");
                h.update(target.as_bytes());
            }
        }
        h.update([b'\n']);
    }
    Ok(ContentHash(h.finalize().into()))
}
```

`lib.rs`: add `pub mod tree;` and `pub use tree::{ContentHash, Stamp, TreeEntry, TreeError};`.

- [ ] **Step 4: Run it and watch it pass**

Run: `cargo test -p rupu-skills --test it tree::`
Expected: 6 passed on macOS and Linux.

- [ ] **Step 5: Commit**

```bash
rustfmt crates/rupu-skills/src/tree.rs crates/rupu-skills/src/lib.rs crates/rupu-skills/tests/it/tree.rs crates/rupu-skills/tests/it/main.rs
git add crates/rupu-skills
git commit -m "feat(skills): walk a skill tree and fingerprint it"
```

---

### Task 4: `load_skill` and the layered catalog

**Files:**
- Create: `crates/rupu-skills/src/skill.rs`, `crates/rupu-skills/src/catalog.rs`
- Modify: `crates/rupu-skills/src/lib.rs`, `crates/rupu-skills/tests/it/main.rs`
- Test: `crates/rupu-skills/tests/it/support.rs`, `crates/rupu-skills/tests/it/catalog.rs`

**Interfaces:**
- Consumes: `parse_skill_md`, `SkillName`, `walk_tree`, `stamp`, `content_hash`.
- Produces:
  - `Layer::{Global, Project}`: `Copy + Ord + Hash + Serialize` (lowercase); `as_str(self) -> &'static str`; `Layer::parse(&str) -> Option<Layer>`.
  - `Skill { name: SkillName, layer: Layer, root: PathBuf /* canonical */, frontmatter: Frontmatter, body: String, entries: Vec<TreeEntry>, stamp: Stamp }`, with `files(&self) -> Vec<&str>` (entry rels) and `content_hash(&self) -> Result<ContentHash, TreeError>`.
  - `load_skill(dir: &Path, layer: Layer) -> Result<Skill, SkillError>`.
  - `SkillError::{MissingSkillMd(PathBuf), Io { path, source }, Frontmatter(FrontmatterError), Name { name: String, source: NameError }, NameMismatch { name: String, dir: String }, ProjectSymlink, Tree(TreeError)}`.
  - `LayerDirs { global: Option<PathBuf>, project: Option<PathBuf> }`: the two `skills/` directories.
  - `SkillCatalog::discover(dirs: &LayerDirs) -> Result<SkillCatalog, DiscoverError>`; `entries(&self) -> &[CatalogEntry]`; `named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a CatalogEntry>`; `get(&self, layer: Layer, name: &str) -> Option<&CatalogEntry>`.
  - `CatalogEntry { dir_name: String, layer: Layer, path: PathBuf, skill: Result<Arc<Skill>, SkillError> }`, with `is_valid(&self) -> bool`.
  - `DiscoverError::Io { path: PathBuf, source: std::io::Error }`.

- [ ] **Step 1: Write the test support and the failing test**

`crates/rupu-skills/tests/it/support.rs` (add `mod support;` and `mod catalog;` to `main.rs`):

```rust
use std::fs;
use std::path::{Path, PathBuf};

/// Writes `<skills_dir>/<dir>/SKILL.md` naming `name`, plus one script.
pub fn write_skill(skills_dir: &Path, dir: &str, name: &str, description: &str) -> PathBuf {
    let root = skills_dir.join(dir);
    fs::create_dir_all(root.join("scripts")).unwrap();
    fs::write(
        root.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {description}\n---\n\nDo the {name} thing.\n"),
    )
    .unwrap();
    fs::write(root.join("scripts/run.sh"), "#!/bin/sh\necho ok\n").unwrap();
    root
}
```

`crates/rupu-skills/tests/it/catalog.rs`:

```rust
use crate::support::write_skill;
use rupu_skills::{load_skill, Layer, LayerDirs, SkillCatalog, SkillError};
use std::fs;

#[test]
fn load_skill_reads_everything() {
    let t = tempfile::tempdir().unwrap();
    let dir = write_skill(t.path(), "pdf-processing", "pdf-processing", "PDF things.");
    let s = load_skill(&dir, Layer::Global).unwrap();
    assert_eq!(s.name.as_str(), "pdf-processing");
    assert_eq!(s.layer, Layer::Global);
    assert_eq!(s.root, dir.canonicalize().unwrap());
    assert_eq!(s.frontmatter.description, "PDF things.");
    assert_eq!(s.body, "Do the pdf-processing thing.");
    assert_eq!(s.files(), vec!["SKILL.md", "scripts/run.sh"]);
    assert_eq!(s.content_hash().unwrap(), s.content_hash().unwrap());
}

#[test]
fn load_skill_errors() {
    let t = tempfile::tempdir().unwrap();
    fs::create_dir_all(t.path().join("empty")).unwrap();
    assert!(matches!(load_skill(&t.path().join("empty"), Layer::Global), Err(SkillError::MissingSkillMd(_))));

    let d = write_skill(t.path(), "dir-name", "other-name", "d");
    assert!(matches!(
        load_skill(&d, Layer::Global),
        Err(SkillError::NameMismatch { name, dir }) if name == "other-name" && dir == "dir-name"
    ));

    let d = write_skill(t.path(), "Bad_Name", "Bad_Name", "d");
    assert!(matches!(load_skill(&d, Layer::Global), Err(SkillError::Name { .. })));
}

#[cfg(unix)]
#[test]
fn symlinked_skill_dir_is_global_only() {
    let t = tempfile::tempdir().unwrap();
    let real = write_skill(&t.path().join("elsewhere"), "linked", "linked", "d");
    let skills = t.path().join("skills");
    fs::create_dir_all(&skills).unwrap();
    std::os::unix::fs::symlink(&real, skills.join("linked")).unwrap();
    assert!(load_skill(&skills.join("linked"), Layer::Global).is_ok());
    assert!(matches!(load_skill(&skills.join("linked"), Layer::Project), Err(SkillError::ProjectSymlink)));
}

#[test]
fn discover_keeps_both_layers_and_invalid_entries() {
    let g = tempfile::tempdir().unwrap();
    let p = tempfile::tempdir().unwrap();
    write_skill(g.path(), "code-review", "code-review", "global review");
    write_skill(g.path(), "pdf-processing", "pdf-processing", "pdf");
    write_skill(p.path(), "code-review", "code-review", "project review");
    write_skill(p.path(), "broken", "not-broken", "d");
    fs::create_dir_all(p.path().join(".hidden")).unwrap();
    fs::write(p.path().join("README.md"), "not a skill").unwrap();

    let cat = SkillCatalog::discover(&LayerDirs {
        global: Some(g.path().to_owned()),
        project: Some(p.path().to_owned()),
    })
    .unwrap();
    let got: Vec<(String, Layer, bool)> = cat
        .entries()
        .iter()
        .map(|e| (e.dir_name.clone(), e.layer, e.is_valid()))
        .collect();
    assert_eq!(
        got,
        vec![
            ("broken".into(), Layer::Project, false),
            ("code-review".into(), Layer::Global, true),
            ("code-review".into(), Layer::Project, true),
            ("pdf-processing".into(), Layer::Global, true),
        ]
    );
    assert_eq!(cat.named("code-review").count(), 2);
    let proj = cat.get(Layer::Project, "code-review").unwrap();
    assert_eq!(proj.skill.as_ref().unwrap().frontmatter.description, "project review");
}

#[test]
fn missing_layer_dirs_are_empty() {
    let t = tempfile::tempdir().unwrap();
    let cat = SkillCatalog::discover(&LayerDirs {
        global: Some(t.path().join("absent")),
        project: None,
    })
    .unwrap();
    assert!(cat.entries().is_empty());
}
```

- [ ] **Step 2: Run it and watch it fail**

Run: `cargo test -p rupu-skills --test it catalog::`
Expected: compile error, `load_skill` not found.

- [ ] **Step 3: Implement `skill.rs`**

```rust
//! One skill directory, loaded and validated.

use crate::frontmatter::{parse_skill_md, Frontmatter, FrontmatterError};
use crate::name::{NameError, SkillName};
use crate::tree::{self, ContentHash, Stamp, TreeEntry, TreeError};
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Layer {
    Global,
    Project,
}

impl Layer {
    pub fn as_str(self) -> &'static str {
        match self {
            Layer::Global => "global",
            Layer::Project => "project",
        }
    }

    pub fn parse(s: &str) -> Option<Layer> {
        match s {
            "global" => Some(Layer::Global),
            "project" => Some(Layer::Project),
            _ => None,
        }
    }
}

#[derive(Debug)]
pub struct Skill {
    pub name: SkillName,
    pub layer: Layer,
    pub root: PathBuf,
    pub frontmatter: Frontmatter,
    pub body: String,
    pub entries: Vec<TreeEntry>,
    pub stamp: Stamp,
}

impl Skill {
    pub fn files(&self) -> Vec<&str> {
        self.entries.iter().map(|e| e.rel.as_str()).collect()
    }

    pub fn content_hash(&self) -> Result<ContentHash, TreeError> {
        tree::content_hash(&self.root, &self.entries)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SkillError {
    #[error("no SKILL.md in {0}")]
    MissingSkillMd(PathBuf),
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
    #[error("SKILL.md: {0}")]
    Frontmatter(#[from] FrontmatterError),
    #[error("name {name:?}: {source}")]
    Name { name: String, source: NameError },
    #[error("name {name:?} does not match its directory {dir:?}")]
    NameMismatch { name: String, dir: String },
    #[error("a project skill directory may not be a symlink")]
    ProjectSymlink,
    #[error(transparent)]
    Tree(#[from] TreeError),
}

pub fn load_skill(dir: &Path, layer: Layer) -> Result<Skill, SkillError> {
    let io = |source| SkillError::Io { path: dir.to_owned(), source };
    if layer == Layer::Project && std::fs::symlink_metadata(dir).map_err(io)?.file_type().is_symlink() {
        return Err(SkillError::ProjectSymlink);
    }
    let root = dir.canonicalize().map_err(io)?;
    let md_path = root.join("SKILL.md");
    let text = match std::fs::read_to_string(&md_path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(SkillError::MissingSkillMd(dir.to_owned())),
        Err(source) => return Err(SkillError::Io { path: md_path, source }),
    };
    let (frontmatter, body) = parse_skill_md(&text)?;
    let name: SkillName = frontmatter
        .name
        .parse()
        .map_err(|source| SkillError::Name { name: frontmatter.name.clone(), source })?;
    let dir_name = dir.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    if name.as_str() != dir_name {
        return Err(SkillError::NameMismatch { name: frontmatter.name.clone(), dir: dir_name.to_owned() });
    }
    let entries = tree::walk_tree(&root)?;
    let stamp = tree::stamp(&entries);
    Ok(Skill { name, layer, root, frontmatter, body, entries, stamp })
}
```

- [ ] **Step 4: Implement `catalog.rs`**

```rust
//! Every skill installed in the global and project layers. Both layers are
//! kept: a project skill never shadows a global one (spec §1.2). Grants
//! decide which one binds (`grant.rs`).

use crate::skill::{load_skill, Layer, Skill, SkillError};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The two `skills/` directories. `project` is `None` when the run has no
/// project layer (see `rupu_workspace::project_layer_dir`).
#[derive(Debug, Clone, Default)]
pub struct LayerDirs {
    pub global: Option<PathBuf>,
    pub project: Option<PathBuf>,
}

#[derive(Debug)]
pub struct CatalogEntry {
    pub dir_name: String,
    pub layer: Layer,
    pub path: PathBuf,
    pub skill: Result<Arc<Skill>, SkillError>,
}

impl CatalogEntry {
    pub fn is_valid(&self) -> bool {
        self.skill.is_ok()
    }
}

#[derive(Debug, Default)]
pub struct SkillCatalog {
    entries: Vec<CatalogEntry>,
}

#[derive(Debug, thiserror::Error)]
pub enum DiscoverError {
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
}

impl SkillCatalog {
    pub fn discover(dirs: &LayerDirs) -> Result<Self, DiscoverError> {
        let mut entries = Vec::new();
        for (dir, layer) in [(&dirs.global, Layer::Global), (&dirs.project, Layer::Project)] {
            if let Some(dir) = dir {
                scan(dir, layer, &mut entries)?;
            }
        }
        entries.sort_by(|a, b| (&a.dir_name, a.layer).cmp(&(&b.dir_name, b.layer)));
        Ok(Self { entries })
    }

    pub fn entries(&self) -> &[CatalogEntry] {
        &self.entries
    }

    pub fn named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a CatalogEntry> {
        self.entries.iter().filter(move |e| e.dir_name == name)
    }

    pub fn get(&self, layer: Layer, name: &str) -> Option<&CatalogEntry> {
        self.entries.iter().find(|e| e.layer == layer && e.dir_name == name)
    }
}

fn scan(dir: &Path, layer: Layer, out: &mut Vec<CatalogEntry>) -> Result<(), DiscoverError> {
    let read = match std::fs::read_dir(dir) {
        Ok(r) => r,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(source) => return Err(DiscoverError::Io { path: dir.to_owned(), source }),
    };
    for item in read {
        let item = item.map_err(|source| DiscoverError::Io { path: dir.to_owned(), source })?;
        let path = item.path();
        let Some(dir_name) = item.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if dir_name.starts_with('.') || !path.is_dir() {
            continue;
        }
        let skill = load_skill(&path, layer).map(Arc::new);
        out.push(CatalogEntry { dir_name, layer, path, skill });
    }
    Ok(())
}
```

`lib.rs`: add `pub mod skill; pub mod catalog;` and `pub use skill::{load_skill, Layer, Skill, SkillError}; pub use catalog::{CatalogEntry, DiscoverError, LayerDirs, SkillCatalog};`.

- [ ] **Step 5: Run it and watch it pass**

Run: `cargo test -p rupu-skills --test it catalog::`
Expected: 5 passed.

- [ ] **Step 6: Commit**

```bash
rustfmt crates/rupu-skills/src/skill.rs crates/rupu-skills/src/catalog.rs crates/rupu-skills/src/lib.rs crates/rupu-skills/tests/it/*.rs
git add crates/rupu-skills
git commit -m "feat(skills): load skills and discover both layers"
```

---

### Task 5: Grant grammar and resolution

**Files:**
- Create: `crates/rupu-skills/src/grant.rs`
- Modify: `crates/rupu-skills/src/lib.rs`, `crates/rupu-skills/tests/it/main.rs`
- Test: `crates/rupu-skills/tests/it/grant.rs`

**Interfaces:**
- Consumes: `SkillCatalog`, `Layer`, `SkillName`, `Skill`.
- Produces:
  - `SkillGrant::{Name(SkillName), Qualified(Layer, SkillName), AllGlobal, AllProject}`: `FromStr<Err = GrantParseError>`, `Debug + Clone + PartialEq + Eq`.
  - `GrantParseError::{Name { entry: String, source: NameError }, UnknownLayer { entry: String, prefix: String }}`.
  - `resolve_grants(grants: &[SkillGrant], catalog: &SkillCatalog) -> Result<Vec<Arc<Skill>>, Vec<ResolveError>>`: deduplicated by `(layer, name)`, sorted by name then layer, reporting every error.
  - `ResolveError::{Unknown { entry: String, suggestion: Option<String> }, Invalid { entry: String, layer: Layer, error: String }, Ambiguous { name: String }}`, implementing `Display` with the messages in Step 1.
  - `parse_grants(entries: &[String]) -> Result<Vec<SkillGrant>, GrantParseError>`.

- [ ] **Step 1: Write the failing test** (`crates/rupu-skills/tests/it/grant.rs`; add `mod grant;`)

```rust
use crate::support::write_skill;
use rupu_skills::{parse_grants, resolve_grants, Layer, LayerDirs, ResolveError, SkillCatalog, SkillGrant};

fn catalog() -> (tempfile::TempDir, tempfile::TempDir, SkillCatalog) {
    let g = tempfile::tempdir().unwrap();
    let p = tempfile::tempdir().unwrap();
    write_skill(g.path(), "code-review", "code-review", "g");
    write_skill(g.path(), "pdf-processing", "pdf-processing", "g");
    write_skill(g.path(), "bad", "not-bad", "g");
    write_skill(p.path(), "code-review", "code-review", "p");
    write_skill(p.path(), "local-only", "local-only", "p");
    let cat = SkillCatalog::discover(&LayerDirs { global: Some(g.path().into()), project: Some(p.path().into()) }).unwrap();
    (g, p, cat)
}

fn names(v: &[std::sync::Arc<rupu_skills::Skill>]) -> Vec<(String, Layer)> {
    v.iter().map(|s| (s.name.to_string(), s.layer)).collect()
}

#[test]
fn parse_grammar() {
    assert_eq!("*".parse::<SkillGrant>().unwrap(), SkillGrant::AllGlobal);
    assert_eq!("global:*".parse::<SkillGrant>().unwrap(), SkillGrant::AllGlobal);
    assert_eq!("project:*".parse::<SkillGrant>().unwrap(), SkillGrant::AllProject);
    assert_eq!(
        "project:code-review".parse::<SkillGrant>().unwrap(),
        SkillGrant::Qualified(Layer::Project, "code-review".parse().unwrap())
    );
    assert_eq!("pdf-processing".parse::<SkillGrant>().unwrap(), SkillGrant::Name("pdf-processing".parse().unwrap()));
    assert!("team:x".parse::<SkillGrant>().is_err());
    assert!("Bad".parse::<SkillGrant>().is_err());
}

#[test]
fn star_is_global_only_and_skips_invalid() {
    let (_g, _p, cat) = catalog();
    let got = resolve_grants(&parse_grants(&["*".into()]).unwrap(), &cat).unwrap();
    assert_eq!(names(&got), vec![("code-review".into(), Layer::Global), ("pdf-processing".into(), Layer::Global)]);
}

#[test]
fn project_skills_need_an_explicit_grant() {
    let (_g, _p, cat) = catalog();
    let got = resolve_grants(&parse_grants(&["project:*".into()]).unwrap(), &cat).unwrap();
    assert_eq!(names(&got), vec![("code-review".into(), Layer::Project), ("local-only".into(), Layer::Project)]);
    let got = resolve_grants(&parse_grants(&["local-only".into()]).unwrap(), &cat).unwrap();
    assert_eq!(names(&got), vec![("local-only".into(), Layer::Project)]);
}

#[test]
fn a_name_in_both_layers_must_be_qualified() {
    let (_g, _p, cat) = catalog();
    let err = resolve_grants(&parse_grants(&["code-review".into()]).unwrap(), &cat).unwrap_err();
    assert_eq!(err, vec![ResolveError::Ambiguous { name: "code-review".into() }]);
    assert_eq!(
        err[0].to_string(),
        "skill \"code-review\" is installed in both the global and the project layer; grant global:code-review or project:code-review"
    );
    let got = resolve_grants(&parse_grants(&["global:code-review".into()]).unwrap(), &cat).unwrap();
    assert_eq!(names(&got), vec![("code-review".into(), Layer::Global)]);
}

#[test]
fn unknown_and_invalid_names_report_every_error() {
    let (_g, _p, cat) = catalog();
    let err = resolve_grants(&parse_grants(&["pdf-procesing".into(), "bad".into(), "nothing-like-it".into()]).unwrap(), &cat).unwrap_err();
    assert_eq!(err.len(), 3);
    assert_eq!(err[0].to_string(), "unknown skill \"pdf-procesing\" (did you mean pdf-processing?)");
    assert!(matches!(&err[1], ResolveError::Invalid { entry, layer: Layer::Global, .. } if entry == "bad"));
    assert_eq!(err[2].to_string(), "unknown skill \"nothing-like-it\"");
}

#[test]
fn duplicates_collapse() {
    let (_g, _p, cat) = catalog();
    let got = resolve_grants(&parse_grants(&["*".into(), "pdf-processing".into(), "global:pdf-processing".into()]).unwrap(), &cat).unwrap();
    assert_eq!(names(&got), vec![("code-review".into(), Layer::Global), ("pdf-processing".into(), Layer::Global)]);
}
```

- [ ] **Step 2: Run it and watch it fail**

Run: `cargo test -p rupu-skills --test it grant::`
Expected: compile error, `SkillGrant` not found.

- [ ] **Step 3: Implement** (`crates/rupu-skills/src/grant.rs`)

```rust
//! The `skills:` grant grammar (spec §2.1). `*` is the global layer only;
//! a project skill needs its exact name or `project:*`; a bare name in
//! both layers is an error until qualified.

use crate::catalog::{CatalogEntry, SkillCatalog};
use crate::name::{NameError, SkillName};
use crate::skill::{Layer, Skill};
use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillGrant {
    Name(SkillName),
    Qualified(Layer, SkillName),
    AllGlobal,
    AllProject,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GrantParseError {
    #[error("skills: entry {entry:?}: {source}")]
    Name { entry: String, source: NameError },
    #[error("skills: entry {entry:?}: unknown layer {prefix:?} (use global: or project:)")]
    UnknownLayer { entry: String, prefix: String },
}

impl FromStr for SkillGrant {
    type Err = GrantParseError;

    fn from_str(entry: &str) -> Result<Self, GrantParseError> {
        let name = |s: &str| {
            s.parse::<SkillName>()
                .map_err(|source| GrantParseError::Name { entry: entry.to_owned(), source })
        };
        if entry == "*" {
            return Ok(SkillGrant::AllGlobal);
        }
        let Some((prefix, rest)) = entry.split_once(':') else {
            return Ok(SkillGrant::Name(name(entry)?));
        };
        let layer = Layer::parse(prefix).ok_or_else(|| GrantParseError::UnknownLayer {
            entry: entry.to_owned(),
            prefix: prefix.to_owned(),
        })?;
        Ok(match (layer, rest) {
            (Layer::Global, "*") => SkillGrant::AllGlobal,
            (Layer::Project, "*") => SkillGrant::AllProject,
            (layer, rest) => SkillGrant::Qualified(layer, name(rest)?),
        })
    }
}

pub fn parse_grants(entries: &[String]) -> Result<Vec<SkillGrant>, GrantParseError> {
    entries.iter().map(|e| e.parse()).collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveError {
    Unknown { entry: String, suggestion: Option<String> },
    Invalid { entry: String, layer: Layer, error: String },
    Ambiguous { name: String },
}

impl fmt::Display for ResolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ResolveError::Unknown { entry, suggestion: Some(s) } => {
                write!(f, "unknown skill {entry:?} (did you mean {s}?)")
            }
            ResolveError::Unknown { entry, suggestion: None } => write!(f, "unknown skill {entry:?}"),
            ResolveError::Invalid { entry, layer, error } => {
                write!(f, "skill {entry:?} ({}) is invalid: {error}", layer.as_str())
            }
            ResolveError::Ambiguous { name } => write!(
                f,
                "skill {name:?} is installed in both the global and the project layer; \
                 grant global:{name} or project:{name}"
            ),
        }
    }
}

impl std::error::Error for ResolveError {}

pub fn resolve_grants(grants: &[SkillGrant], catalog: &SkillCatalog) -> Result<Vec<Arc<Skill>>, Vec<ResolveError>> {
    let mut picked: BTreeMap<(String, Layer), Arc<Skill>> = BTreeMap::new();
    let mut errors = Vec::new();
    let mut take = |e: &CatalogEntry, entry: &str, errors: &mut Vec<ResolveError>| match &e.skill {
        Ok(s) => {
            picked.insert((e.dir_name.clone(), e.layer), s.clone());
        }
        Err(err) => errors.push(ResolveError::Invalid {
            entry: entry.to_owned(),
            layer: e.layer,
            error: err.to_string(),
        }),
    };
    for g in grants {
        match g {
            SkillGrant::AllGlobal | SkillGrant::AllProject => {
                let layer = if *g == SkillGrant::AllGlobal { Layer::Global } else { Layer::Project };
                for e in catalog.entries().iter().filter(|e| e.layer == layer && e.is_valid()) {
                    take(e, &e.dir_name, &mut errors);
                }
            }
            SkillGrant::Qualified(layer, name) => match catalog.get(*layer, name.as_str()) {
                Some(e) => take(e, &format!("{}:{name}", layer.as_str()), &mut errors),
                None => errors.push(unknown(&format!("{}:{name}", layer.as_str()), name.as_str(), catalog)),
            },
            SkillGrant::Name(name) => {
                let found: Vec<&CatalogEntry> = catalog.named(name.as_str()).collect();
                match found.as_slice() {
                    [] => errors.push(unknown(name.as_str(), name.as_str(), catalog)),
                    [one] => take(one, name.as_str(), &mut errors),
                    _ => errors.push(ResolveError::Ambiguous { name: name.to_string() }),
                }
            }
        }
    }
    if errors.is_empty() {
        Ok(picked.into_values().collect())
    } else {
        Err(errors)
    }
}

fn unknown(entry: &str, name: &str, catalog: &SkillCatalog) -> ResolveError {
    let limit = (name.len() / 3).max(2);
    let suggestion = catalog
        .entries()
        .iter()
        .map(|e| (edit_distance(name, &e.dir_name), &e.dir_name))
        .filter(|(d, _)| *d <= limit)
        .min()
        .map(|(_, n)| n.clone());
    ResolveError::Unknown { entry: entry.to_owned(), suggestion }
}

fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let sub = prev[j] + usize::from(ca != *cb);
            cur.push(sub.min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}
```

Note: `picked` is keyed by `(name, layer)`, so `into_values()` yields name-then-layer order (`Layer` derives `Ord` with `Global < Project`).

`lib.rs`: add `pub mod grant;` and `pub use grant::{parse_grants, resolve_grants, GrantParseError, ResolveError, SkillGrant};`.

- [ ] **Step 4: Run it and watch it pass**

Run: `cargo test -p rupu-skills --test it grant::`
Expected: 6 passed.

- [ ] **Step 5: Commit**

```bash
rustfmt crates/rupu-skills/src/grant.rs crates/rupu-skills/src/lib.rs crates/rupu-skills/tests/it/grant.rs crates/rupu-skills/tests/it/main.rs
git add crates/rupu-skills
git commit -m "feat(skills): grant grammar with a global-only wildcard"
```

---

### Task 6: Prompt renderers and the `allowed-tools` map

**Files:**
- Create: `crates/rupu-skills/src/render.rs`, `crates/rupu-skills/src/tools_map.rs`
- Modify: `crates/rupu-skills/src/lib.rs`, `crates/rupu-skills/tests/it/main.rs`
- Test: `crates/rupu-skills/tests/it/render.rs`

**Interfaces:**
- Consumes: `ContentHash`.
- Produces (Plans 2 and 3 call these; their text is the contract):
  - `IndexItem<'a> { name: &'a str, description: &'a str }`, `PreloadItem<'a> { name: &'a str, base_dir: &'a Path, body: &'a str }`.
  - `render_skills_section(preloads: &[PreloadItem<'_>], index: &[IndexItem<'_>]) -> Option<String>`: `None` when both are empty. Items are sorted by name, so the output is byte-stable for a set.
  - `LoadView<'a> { name: &'a str, version: ContentHash, base_dir: &'a Path, files: &'a [&'a str], compatibility: Option<&'a str>, changed_note: Option<&'a str>, body: &'a str }`.
  - `render_load_result(v: &LoadView<'_>) -> String`.
  - `mapped_tools(allowed: &[String]) -> Vec<&'static str>`: rupu core tool names, deduplicated, sorted.

- [ ] **Step 1: Write the failing test** (`crates/rupu-skills/tests/it/render.rs`; add `mod render;`)

```rust
use rupu_skills::render::{render_load_result, render_skills_section, IndexItem, LoadView, PreloadItem};
use rupu_skills::tools_map::mapped_tools;
use rupu_skills::ContentHash;
use std::path::Path;

#[test]
fn nothing_renders_nothing() {
    assert_eq!(render_skills_section(&[], &[]), None);
}

#[test]
fn index_is_sorted_and_exact() {
    let got = render_skills_section(
        &[],
        &[
            IndexItem { name: "pdf-processing", description: "Extract PDF text. Use when handling PDFs." },
            IndexItem { name: "code-review", description: "Review a diff." },
        ],
    )
    .unwrap();
    assert_eq!(
        got,
        "## Skills\n\n\
Each skill below holds instructions, and sometimes scripts and reference files,\n\
for one kind of task. When a task matches a skill's description, call\n\
`skills.load` with its name before you start, and follow it. Paths inside a\n\
skill are relative to the base directory that `skills.load` returns.\n\n\
- code-review: Review a diff.\n\
- pdf-processing: Extract PDF text. Use when handling PDFs."
    );
}

#[test]
fn preloads_come_first_then_the_index() {
    let got = render_skills_section(
        &[PreloadItem { name: "house-style", base_dir: Path::new("/h/.rupu/skills/house-style"), body: "Write plainly." }],
        &[IndexItem { name: "code-review", description: "Review a diff." }],
    )
    .unwrap();
    assert!(got.starts_with("## Skill: house-style\n\nBase directory: /h/.rupu/skills/house-style\n\nWrite plainly.\n\n## Skills\n\n"));
    assert!(got.ends_with("- code-review: Review a diff."));
}

#[test]
fn load_result_is_exact() {
    let got = render_load_result(&LoadView {
        name: "pdf-processing",
        version: ContentHash([0xab; 32]),
        base_dir: Path::new("/h/.rupu/skills/pdf-processing"),
        files: &["SKILL.md", "scripts/extract.py"],
        compatibility: Some("Requires Python 3.12+"),
        changed_note: Some("This skill changed on disk since this run started; this is its current version."),
        body: "Step one.",
    });
    assert_eq!(
        got,
        "This skill changed on disk since this run started; this is its current version.\n\
Skill: pdf-processing (version abababababab)\n\
Base directory: /h/.rupu/skills/pdf-processing\n\
Files: SKILL.md, scripts/extract.py\n\
Compatibility: Requires Python 3.12+\n\n\
Step one."
    );
}

#[test]
fn allowed_tools_map_to_core_names() {
    let allowed: Vec<String> = ["Bash(git status:*)", "Read", "Grep", "Glob", "Edit", "MultiEdit", "Write", "WebFetch", "mcp__x__y", "Bash(jq:*)"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(mapped_tools(&allowed), vec!["bash", "edit_file", "glob", "grep", "read_file", "write_file"]);
}
```

- [ ] **Step 2: Run it and watch it fail**

Run: `cargo test -p rupu-skills --test it render::`
Expected: compile error, `render` not found.

- [ ] **Step 3: Implement `render.rs`**

```rust
//! The text a model sees (spec §3.1 to §3.3). Changing a string here
//! changes every run's system prompt; the tests pin each byte.

use crate::tree::ContentHash;
use std::path::Path;

pub struct IndexItem<'a> {
    pub name: &'a str,
    pub description: &'a str,
}

pub struct PreloadItem<'a> {
    pub name: &'a str,
    pub base_dir: &'a Path,
    pub body: &'a str,
}

const INDEX_HEADER: &str = "## Skills\n\n\
Each skill below holds instructions, and sometimes scripts and reference files,\n\
for one kind of task. When a task matches a skill's description, call\n\
`skills.load` with its name before you start, and follow it. Paths inside a\n\
skill are relative to the base directory that `skills.load` returns.";

pub fn render_skills_section(preloads: &[PreloadItem<'_>], index: &[IndexItem<'_>]) -> Option<String> {
    let mut parts = Vec::new();
    let mut preloads: Vec<&PreloadItem<'_>> = preloads.iter().collect();
    preloads.sort_by_key(|p| p.name);
    for p in preloads {
        parts.push(format!(
            "## Skill: {}\n\nBase directory: {}\n\n{}",
            p.name,
            p.base_dir.display(),
            p.body
        ));
    }
    if !index.is_empty() {
        let mut index: Vec<&IndexItem<'_>> = index.iter().collect();
        index.sort_by_key(|i| i.name);
        let lines: Vec<String> = index.iter().map(|i| format!("- {}: {}", i.name, i.description)).collect();
        parts.push(format!("{INDEX_HEADER}\n\n{}", lines.join("\n")));
    }
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

pub struct LoadView<'a> {
    pub name: &'a str,
    pub version: ContentHash,
    pub base_dir: &'a Path,
    pub files: &'a [&'a str],
    pub compatibility: Option<&'a str>,
    pub changed_note: Option<&'a str>,
    pub body: &'a str,
}

pub fn render_load_result(v: &LoadView<'_>) -> String {
    let mut out = String::new();
    if let Some(note) = v.changed_note {
        out.push_str(note);
        out.push('\n');
    }
    out.push_str(&format!("Skill: {} (version {})\n", v.name, v.version.short()));
    out.push_str(&format!("Base directory: {}\n", v.base_dir.display()));
    out.push_str(&format!("Files: {}\n", v.files.join(", ")));
    if let Some(c) = v.compatibility {
        out.push_str(&format!("Compatibility: {c}\n"));
    }
    out.push('\n');
    out.push_str(v.body);
    out
}
```

- [ ] **Step 4: Implement `tools_map.rs`**

```rust
//! `allowed-tools` names from other harnesses, mapped to rupu's core
//! tools. Used only to warn when a run lacks a tool a skill expects
//! (spec §3.5); it never grants anything.

const MAP: &[(&str, &str)] = &[
    ("Bash", "bash"),
    ("Read", "read_file"),
    ("Write", "write_file"),
    ("Edit", "edit_file"),
    ("MultiEdit", "edit_file"),
    ("Grep", "grep"),
    ("Glob", "glob"),
];

pub fn mapped_tools(allowed: &[String]) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = allowed
        .iter()
        .filter_map(|entry| {
            let base = entry.split('(').next().unwrap_or(entry).trim();
            MAP.iter().find(|(from, _)| *from == base).map(|(_, to)| *to)
        })
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}
```

`lib.rs`: add `pub mod render; pub mod tools_map;`.

- [ ] **Step 5: Run it and watch it pass**

Run: `cargo test -p rupu-skills --test it render::`
Expected: 5 passed.

- [ ] **Step 6: Commit**

```bash
rustfmt crates/rupu-skills/src/render.rs crates/rupu-skills/src/tools_map.rs crates/rupu-skills/src/lib.rs crates/rupu-skills/tests/it/render.rs crates/rupu-skills/tests/it/main.rs
git add crates/rupu-skills
git commit -m "feat(skills): render the skills section and load result"
```

---

### Task 7: Find, choose and install skills from a directory

**Files:**
- Create: `crates/rupu-skills/src/install.rs`
- Modify: `crates/rupu-skills/src/lib.rs`, `crates/rupu-skills/tests/it/main.rs`
- Test: `crates/rupu-skills/tests/it/install.rs`

**Interfaces:**
- Consumes: `parse_skill_md`, `SkillName`, `load_skill`, `Layer`, `walk_tree`, `content_hash`, `EntryKind`, `Stamp`, `ContentHash`.
- Produces:
  - `MAX_SEARCH_DEPTH: usize = 6`.
  - `Found { name: String, rel: String /* "" when the root is the skill */, path: PathBuf, description: Option<String>, error: Option<String> }`. `name` is the frontmatter `name` when it parses, else the directory name.
  - `find_skills(root: &Path) -> Result<Vec<Found>, InstallError>`, sorted by `rel`.
  - `Selection::{Default, Named(Vec<String>), All}`.
  - `choose<'a>(found: &'a [Found], sel: &Selection) -> Result<Vec<&'a Found>, InstallError>`.
  - `install_selected(found: &[Found], sel: &Selection, skills_dir: &Path, layer: Layer, force: bool) -> Result<InstallReport, InstallError>`.
  - `install_one(source_dir: &Path, skills_dir: &Path, layer: Layer, force: bool) -> Result<Installed, InstallError>`.
  - `source_hash(source_dir: &Path) -> Result<(SkillName, ContentHash), InstallError>`: validates a source skill and returns its name and content hash (Task 9 uses it).
  - `InstallReport { installed: Vec<Installed>, skipped: Vec<(String, String)> }`.
  - `Installed { name: SkillName, path: PathBuf, outcome: InstallOutcome, stamp: Stamp, content_hash: ContentHash, rel: String }`.
  - `InstallOutcome::{Created, Replaced}`.
  - `InstallError::{NoSkill(PathBuf), Several(Vec<String>), NameNotFound { name: String, found: Vec<String> }, DuplicateName { name: String, paths: Vec<String> }, Invalid { name: String, error: String }, AbsoluteSymlink { skill: String, rel: String }, Exists(PathBuf), Io { path: PathBuf, source: std::io::Error }}`. Task 8 adds `Source(SourceError)`.

- [ ] **Step 1: Write the failing test** (`crates/rupu-skills/tests/it/install.rs`; add `mod install;`)

```rust
use crate::support::write_skill;
use rupu_skills::install::{
    choose, find_skills, install_one, install_selected, InstallError, InstallOutcome, Selection,
};
use rupu_skills::{load_skill, Layer};
use std::fs;
use std::path::Path;

/// A repo shaped like the published skill collections: `skills/<name>/`,
/// a template at the root, skills under `.claude/skills/`, and noise.
fn collection(root: &Path) {
    write_skill(&root.join("skills"), "pdf", "pdf", "PDF things.");
    write_skill(&root.join("skills"), "docx", "docx", "Word things.");
    write_skill(&root.join("skills/pdf"), "nested", "nested", "never found: inside a skill");
    write_skill(root, "template", "template-skill", "A starting point.");
    write_skill(&root.join(".claude/skills"), "hidden-one", "hidden-one", "From a plugin folder.");
    write_skill(&root.join("node_modules"), "dep", "dep", "never found: node_modules");
    write_skill(&root.join(".git"), "obj", "obj", "never found: .git");
    fs::write(root.join("README.md"), "# skills\n").unwrap();
}

#[test]
fn find_skills_searches_like_the_ecosystem() {
    let t = tempfile::tempdir().unwrap();
    collection(t.path());
    let found = find_skills(t.path()).unwrap();
    let got: Vec<(&str, &str)> = found.iter().map(|f| (f.name.as_str(), f.rel.as_str())).collect();
    assert_eq!(
        got,
        vec![
            ("hidden-one", ".claude/skills/hidden-one"),
            ("docx", "skills/docx"),
            ("pdf", "skills/pdf"),
            ("template-skill", "template"),
        ]
    );
    assert_eq!(found[2].description.as_deref(), Some("PDF things."));
}

#[test]
fn a_root_skill_is_found_by_its_frontmatter_name() {
    let t = tempfile::tempdir().unwrap();
    let root = write_skill(t.path(), "repo-checkout", "report-style", "d");
    let found = find_skills(&root).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!((found[0].name.as_str(), found[0].rel.as_str()), ("report-style", ""));
}

#[test]
fn search_depth_is_bounded() {
    let t = tempfile::tempdir().unwrap();
    write_skill(&t.path().join("a/b/c/d/e/f"), "deep", "deep", "seven levels down");
    write_skill(&t.path().join("a/b/c/d/e"), "ok", "ok", "six levels down");
    let names: Vec<String> = find_skills(t.path()).unwrap().into_iter().map(|f| f.name).collect();
    assert_eq!(names, vec!["ok"]);
}

#[test]
fn duplicate_names_in_one_source_are_an_error() {
    let t = tempfile::tempdir().unwrap();
    write_skill(&t.path().join("a"), "same", "same", "d");
    write_skill(&t.path().join("b"), "same", "same", "d");
    assert!(matches!(
        find_skills(t.path()),
        Err(InstallError::DuplicateName { name, paths }) if name == "same" && paths == ["a/same", "b/same"]
    ));
}

#[test]
fn choose_by_selection() {
    let t = tempfile::tempdir().unwrap();
    collection(t.path());
    let found = find_skills(t.path()).unwrap();
    assert!(matches!(choose(&found, &Selection::Default), Err(InstallError::Several(v)) if v.len() == 4));
    let one = choose(&found, &Selection::Named(vec!["pdf".into()])).unwrap();
    assert_eq!(one[0].rel, "skills/pdf");
    assert_eq!(choose(&found, &Selection::All).unwrap().len(), 4);
    assert!(matches!(
        choose(&found, &Selection::Named(vec!["xlsx".into()])),
        Err(InstallError::NameNotFound { name, .. }) if name == "xlsx"
    ));
    let single = find_skills(&t.path().join("skills/pdf")).unwrap();
    assert_eq!(choose(&single, &Selection::Default).unwrap().len(), 1);
}

#[test]
fn install_one_names_the_skill_by_its_frontmatter() {
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    let root = write_skill(src.path(), "repo-checkout", "report-style", "d");
    fs::create_dir_all(root.join(".git")).unwrap();
    fs::write(root.join(".git/HEAD"), "x").unwrap();
    let got = install_one(&root, dst.path(), Layer::Global, false).unwrap();
    assert_eq!(got.outcome, InstallOutcome::Created);
    assert_eq!(got.path, dst.path().join("report-style"));
    let installed = load_skill(&got.path, Layer::Global).unwrap();
    assert_eq!(installed.files(), vec!["SKILL.md", "scripts/run.sh"]);
    assert_eq!(installed.content_hash().unwrap(), got.content_hash);
    let leftovers: Vec<_> = fs::read_dir(dst.path()).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert_eq!(leftovers, vec![std::ffi::OsString::from("report-style")], "no staging dirs left");
}

#[cfg(unix)]
#[test]
fn keeps_the_exec_bit() {
    use std::os::unix::fs::PermissionsExt;
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    let skill = write_skill(src.path(), "s", "s", "d");
    fs::set_permissions(skill.join("scripts/run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
    let got = install_one(&skill, dst.path(), Layer::Global, false).unwrap();
    let mode = fs::metadata(got.path.join("scripts/run.sh")).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o755);
}

#[test]
fn refuses_existing_without_force_and_replaces_with_it() {
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    let skill = write_skill(src.path(), "s", "s", "first");
    install_one(&skill, dst.path(), Layer::Global, false).unwrap();
    write_skill(src.path(), "s", "s", "second");
    assert!(matches!(install_one(&skill, dst.path(), Layer::Global, false), Err(InstallError::Exists(_))));
    assert_eq!(install_one(&skill, dst.path(), Layer::Global, true).unwrap().outcome, InstallOutcome::Replaced);
    assert_eq!(load_skill(&dst.path().join("s"), Layer::Global).unwrap().frontmatter.description, "second");
    let leftovers: Vec<_> = fs::read_dir(dst.path()).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert_eq!(leftovers, vec![std::ffi::OsString::from("s")], "no staging or backup dirs left");
}

#[test]
fn a_broken_replacement_leaves_the_old_skill_intact() {
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    let skill = write_skill(src.path(), "s", "s", "good");
    install_one(&skill, dst.path(), Layer::Global, false).unwrap();
    fs::write(skill.join("SKILL.md"), "---\nname: s\n---\n").unwrap();
    assert!(matches!(install_one(&skill, dst.path(), Layer::Global, true), Err(InstallError::Invalid { .. })));
    assert_eq!(load_skill(&dst.path().join("s"), Layer::Global).unwrap().frontmatter.description, "good");
}

#[cfg(unix)]
#[test]
fn absolute_internal_symlinks_are_refused_relative_ones_copied() {
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    let skill = write_skill(src.path(), "s", "s", "d");
    std::os::unix::fs::symlink("scripts/run.sh", skill.join("run")).unwrap();
    let got = install_one(&skill, dst.path(), Layer::Global, false).unwrap();
    assert_eq!(fs::read_link(got.path.join("run")).unwrap(), Path::new("scripts/run.sh"));

    let skill2 = write_skill(src.path(), "t", "t", "d");
    std::os::unix::fs::symlink(skill2.join("scripts/run.sh").canonicalize().unwrap(), skill2.join("run")).unwrap();
    assert!(matches!(
        install_one(&skill2, dst.path(), Layer::Global, false),
        Err(InstallError::AbsoluteSymlink { skill, rel }) if skill == "t" && rel == "run"
    ));
}

#[test]
fn install_all_skips_invalid_named_invalid_fails_before_writing() {
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    write_skill(src.path(), "good", "good", "d");
    let bad = write_skill(src.path(), "bad", "bad", "d");
    fs::write(bad.join("SKILL.md"), "---\nname: bad\n---\n").unwrap();
    let found = find_skills(src.path()).unwrap();

    let err = install_selected(&found, &Selection::Named(vec!["good".into(), "bad".into()]), dst.path(), Layer::Global, false);
    assert!(matches!(err, Err(InstallError::Invalid { name, .. }) if name == "bad"));
    assert!(!dst.path().join("good").exists(), "nothing is written when a named skill is invalid");

    let report = install_selected(&found, &Selection::All, dst.path(), Layer::Global, false).unwrap();
    assert_eq!(report.installed.iter().map(|i| i.name.as_str()).collect::<Vec<_>>(), vec!["good"]);
    assert_eq!(report.skipped.len(), 1);
    assert_eq!(report.skipped[0].0, "bad");
}

#[test]
fn install_selected_checks_every_destination_before_writing() {
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    write_skill(src.path(), "a-one", "a-one", "d");
    write_skill(src.path(), "b-two", "b-two", "d");
    write_skill(dst.path(), "b-two", "b-two", "already here");
    let found = find_skills(src.path()).unwrap();
    assert!(matches!(install_selected(&found, &Selection::All, dst.path(), Layer::Global, false), Err(InstallError::Exists(_))));
    assert!(!dst.path().join("a-one").exists());
}
```

- [ ] **Step 2: Run it and watch it fail**

Run: `cargo test -p rupu-skills --test it install::`
Expected: compile error, `install` not found.

- [ ] **Step 3: Implement** (`crates/rupu-skills/src/install.rs`)

```rust
//! Find the skills in a source directory, choose some, and copy them into
//! a layer's `skills/` directory (spec §7.2). Every chosen skill is
//! validated, and every destination checked, before anything is written.
//! Each copy is staged under its real name, validated again in place, and
//! swapped in by rename, so a failed install never damages an installed
//! skill.

use crate::frontmatter::parse_skill_md;
use crate::name::SkillName;
use crate::skill::{load_skill, Layer};
use crate::tree::{self, ContentHash, EntryKind, Stamp, TreeEntry};
use std::path::{Path, PathBuf};

pub const MAX_SEARCH_DEPTH: usize = 6;
const SKIP_DIRS: &[&str] = &[".git", "node_modules"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub name: String,
    pub rel: String,
    pub path: PathBuf,
    pub description: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selection {
    Default,
    Named(Vec<String>),
    All,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallOutcome {
    Created,
    Replaced,
}

#[derive(Debug)]
pub struct Installed {
    pub name: SkillName,
    pub path: PathBuf,
    pub outcome: InstallOutcome,
    pub stamp: Stamp,
    pub content_hash: ContentHash,
    pub rel: String,
}

#[derive(Debug, Default)]
pub struct InstallReport {
    pub installed: Vec<Installed>,
    pub skipped: Vec<(String, String)>,
}

#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("no SKILL.md found in {0}")]
    NoSkill(PathBuf),
    #[error("the source holds several skills ({}); pick with --skill <name> or take them all with --all", .0.join(", "))]
    Several(Vec<String>),
    #[error("no skill named {name:?} in the source (found: {})", found.join(", "))]
    NameNotFound { name: String, found: Vec<String> },
    #[error("two skills named {name:?} in the source ({}); narrow the source with #<path>", paths.join(", "))]
    DuplicateName { name: String, paths: Vec<String> },
    #[error("skill {name:?} is invalid: {error}")]
    Invalid { name: String, error: String },
    #[error("skill {skill:?}: symlink {rel} uses an absolute target; make it relative or replace it with the file")]
    AbsoluteSymlink { skill: String, rel: String },
    #[error("{0} already exists; pass --force to replace it")]
    Exists(PathBuf),
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> InstallError + '_ {
    move |source| InstallError::Io { path: path.to_owned(), source }
}

pub fn find_skills(root: &Path) -> Result<Vec<Found>, InstallError> {
    let mut out = Vec::new();
    search(root, "", 0, &mut out)?;
    out.sort_by(|a, b| a.rel.cmp(&b.rel));
    let mut by_name: std::collections::BTreeMap<&str, Vec<&str>> = Default::default();
    for f in &out {
        by_name.entry(&f.name).or_default().push(&f.rel);
    }
    if let Some((name, paths)) = by_name.into_iter().find(|(_, p)| p.len() > 1) {
        return Err(InstallError::DuplicateName {
            name: name.to_owned(),
            paths: paths.into_iter().map(str::to_owned).collect(),
        });
    }
    Ok(out)
}

fn search(dir: &Path, rel: &str, depth: usize, out: &mut Vec<Found>) -> Result<(), InstallError> {
    let md = dir.join("SKILL.md");
    if md.is_file() {
        out.push(describe(dir, rel, &md));
        return Ok(());
    }
    if depth == MAX_SEARCH_DEPTH {
        return Ok(());
    }
    for item in std::fs::read_dir(dir).map_err(io(dir))? {
        let item = item.map_err(io(dir))?;
        let Some(name) = item.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if SKIP_DIRS.contains(&name.as_str()) || !item.file_type().map_err(io(dir))?.is_dir() {
            continue;
        }
        let child_rel = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
        search(&item.path(), &child_rel, depth + 1, out)?;
    }
    Ok(())
}

fn describe(dir: &Path, rel: &str, md: &Path) -> Found {
    let dir_name = dir.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_owned();
    let parsed = std::fs::read_to_string(md)
        .map_err(|e| e.to_string())
        .and_then(|t| parse_skill_md(&t).map_err(|e| e.to_string()));
    match parsed {
        Ok((fm, _)) => Found {
            name: fm.name,
            rel: rel.to_owned(),
            path: dir.to_owned(),
            description: Some(fm.description),
            error: None,
        },
        Err(e) => Found { name: dir_name, rel: rel.to_owned(), path: dir.to_owned(), description: None, error: Some(e) },
    }
}

pub fn choose<'a>(found: &'a [Found], sel: &Selection) -> Result<Vec<&'a Found>, InstallError> {
    let names = || found.iter().map(|f| f.name.clone()).collect::<Vec<_>>();
    match sel {
        Selection::All => Ok(found.iter().collect()),
        Selection::Default => match found {
            [] => Err(InstallError::Several(Vec::new())),
            [one] => Ok(vec![one]),
            _ => Err(InstallError::Several(names())),
        },
        Selection::Named(wanted) => wanted
            .iter()
            .map(|w| {
                found.iter().find(|f| &f.name == w).ok_or_else(|| InstallError::NameNotFound {
                    name: w.clone(),
                    found: names(),
                })
            })
            .collect(),
    }
}

/// Validate a source skill: frontmatter, name, tree (no escaping symlinks)
/// and no absolute symlinks. Returns its name and the entries to copy.
fn check_source(dir: &Path) -> Result<(SkillName, Vec<TreeEntry>), InstallError> {
    let fallback = dir.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_owned();
    let invalid = |error: String| InstallError::Invalid { name: fallback.clone(), error };
    let text = std::fs::read_to_string(dir.join("SKILL.md")).map_err(|e| invalid(e.to_string()))?;
    let (fm, _) = parse_skill_md(&text).map_err(|e| invalid(e.to_string()))?;
    let name: SkillName = fm.name.parse().map_err(|e: crate::NameError| invalid(e.to_string()))?;
    let entries = tree::walk_tree(dir).map_err(|e| InstallError::Invalid { name: name.to_string(), error: e.to_string() })?;
    for e in &entries {
        if let EntryKind::Symlink { target } = &e.kind {
            if Path::new(target).is_absolute() {
                return Err(InstallError::AbsoluteSymlink { skill: name.to_string(), rel: e.rel.clone() });
            }
        }
    }
    Ok((name, entries))
}

pub fn source_hash(source_dir: &Path) -> Result<(SkillName, ContentHash), InstallError> {
    let (name, entries) = check_source(source_dir)?;
    let root = source_dir.canonicalize().map_err(io(source_dir))?;
    let hash = tree::content_hash(&root, &entries)
        .map_err(|e| InstallError::Invalid { name: name.to_string(), error: e.to_string() })?;
    Ok((name, hash))
}

pub fn install_selected(
    found: &[Found],
    sel: &Selection,
    skills_dir: &Path,
    layer: Layer,
    force: bool,
) -> Result<InstallReport, InstallError> {
    let mut report = InstallReport::default();
    let mut ready = Vec::new();
    for f in choose(found, sel)? {
        match check_source(&f.path) {
            Ok((name, _)) => ready.push((f, name)),
            Err(e) if *sel == Selection::All => report.skipped.push((f.name.clone(), e.to_string())),
            Err(e) => return Err(e),
        }
    }
    if !force {
        if let Some((_, name)) = ready.iter().find(|(_, n)| skills_dir.join(n.as_str()).exists()) {
            return Err(InstallError::Exists(skills_dir.join(name.as_str())));
        }
    }
    for (f, _) in ready {
        let mut installed = install_one(&f.path, skills_dir, layer, force)?;
        installed.rel = f.rel.clone();
        report.installed.push(installed);
    }
    Ok(report)
}

pub fn install_one(source_dir: &Path, skills_dir: &Path, layer: Layer, force: bool) -> Result<Installed, InstallError> {
    let (name, entries) = check_source(source_dir)?;
    let root = source_dir.canonicalize().map_err(io(source_dir))?;
    let dest = skills_dir.join(name.as_str());
    let exists = dest.exists();
    if exists && !force {
        return Err(InstallError::Exists(dest));
    }
    std::fs::create_dir_all(skills_dir).map_err(io(skills_dir))?;
    let pid = std::process::id();
    let staging = skills_dir.join(format!(".staging-{name}-{pid}"));
    let _ = std::fs::remove_dir_all(&staging);
    let staged = staging.join(name.as_str());
    let result = copy_tree(&root, &entries, &staged).and_then(|()| {
        load_skill(&staged, layer).map_err(|e| InstallError::Invalid { name: name.to_string(), error: e.to_string() })
    });
    let staged_skill = match result {
        Ok(s) => s,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(e);
        }
    };
    let content_hash = staged_skill.content_hash().map_err(|e| InstallError::Invalid {
        name: name.to_string(),
        error: e.to_string(),
    })?;
    let outcome = if exists {
        let backup = skills_dir.join(format!(".replaced-{name}-{pid}"));
        std::fs::rename(&dest, &backup).map_err(io(&dest))?;
        if let Err(source) = std::fs::rename(&staged, &dest) {
            let _ = std::fs::rename(&backup, &dest);
            let _ = std::fs::remove_dir_all(&staging);
            return Err(InstallError::Io { path: dest, source });
        }
        let _ = std::fs::remove_dir_all(&backup);
        InstallOutcome::Replaced
    } else {
        std::fs::rename(&staged, &dest).map_err(io(&dest))?;
        InstallOutcome::Created
    };
    let _ = std::fs::remove_dir_all(&staging);
    let installed = load_skill(&dest, layer).map_err(|e| InstallError::Invalid { name: name.to_string(), error: e.to_string() })?;
    Ok(Installed { name, path: dest, outcome, stamp: installed.stamp, content_hash, rel: String::new() })
}

fn copy_tree(root: &Path, entries: &[TreeEntry], to: &Path) -> Result<(), InstallError> {
    std::fs::create_dir_all(to).map_err(io(to))?;
    for e in entries {
        let dest = to.join(&e.rel);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(io(parent))?;
        }
        match &e.kind {
            EntryKind::File { .. } => {
                std::fs::copy(root.join(&e.rel), &dest).map_err(io(&dest))?;
            }
            EntryKind::Symlink { target } => symlink(target, &dest)?,
        }
    }
    Ok(())
}

#[cfg(unix)]
fn symlink(target: &str, at: &Path) -> Result<(), InstallError> {
    std::os::unix::fs::symlink(target, at).map_err(io(at))
}

#[cfg(not(unix))]
fn symlink(target: &str, at: &Path) -> Result<(), InstallError> {
    Err(InstallError::AbsoluteSymlink { skill: at.display().to_string(), rel: target.to_owned() })
}
```

`std::fs::copy` copies permission bits on Unix, which keeps the exec bit. `walk_tree` already skips `.git`, so `copy_tree` never sees it. Staging and backup directories start with `.`, so `SkillCatalog::discover` never lists one left by a crash.

`lib.rs`: add `pub mod install;`.

- [ ] **Step 4: Run it and watch it pass**

Run: `cargo test -p rupu-skills --test it install::`
Expected: 12 passed.

- [ ] **Step 5: Commit**

```bash
rustfmt crates/rupu-skills/src/install.rs crates/rupu-skills/src/lib.rs crates/rupu-skills/tests/it/install.rs crates/rupu-skills/tests/it/main.rs
git add crates/rupu-skills
git commit -m "feat(skills): find, choose and install skills from a directory"
```

---

### Task 8: Install sources and the git fetcher

**Files:**
- Create: `crates/rupu-skills/src/source.rs`, `crates/rupu-skills/src/git.rs`
- Modify: `crates/rupu-skills/src/lib.rs`, `crates/rupu-skills/src/install.rs` (add `InstallError::Source`), `crates/rupu-skills/tests/it/main.rs`
- Test: `crates/rupu-skills/tests/it/source.rs`, unit tests in `git.rs`

**Interfaces:**
- Produces:
  - `Source::{Local(PathBuf), Git(GitSource)}`; `GitSource { url: String, git_ref: Option<String>, path: Option<String> }`; both `Debug + Clone + PartialEq + Eq`.
  - `parse_source(input: &str, git_ref: Option<&str>, home: Option<&Path>) -> Result<Source, SourceError>`. `home` expands a leading `~/`.
  - `SourceError::{NotFound(String), UnsupportedScheme(String), BadPath(String), RefConflict { url_ref: String, flag_ref: String }, Unrecognized(String), Clone(String), PathMissing(String)}`.
  - `FetchedTree { root: PathBuf, commit: Option<String> }` plus a private temp-dir guard; `FetchedTree::local(root: PathBuf) -> Self`; `FetchedTree::with_guard(root: PathBuf, commit: Option<String>, guard: tempfile::TempDir) -> Self`.
  - `trait Fetch { fn fetch(&self, src: &GitSource) -> Result<FetchedTree, SourceError>; }`.
  - Feature `git`: `git::GitFetcher` (unit struct) implementing `Fetch`. It refuses any URL that isn't `https://`.
  - `InstallError::Source(#[from] SourceError)`.

- [ ] **Step 1: Write the failing test** (`crates/rupu-skills/tests/it/source.rs`; add `mod source;`)

```rust
use rupu_skills::source::{parse_source, GitSource, Source, SourceError};

fn git(url: &str, git_ref: Option<&str>, path: Option<&str>) -> Source {
    Source::Git(GitSource {
        url: url.into(),
        git_ref: git_ref.map(Into::into),
        path: path.map(Into::into),
    })
}

#[test]
fn local_paths() {
    let t = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(t.path().join("skills/x")).unwrap();
    let canon = t.path().join("skills/x").canonicalize().unwrap();
    let p = t.path().join("skills/x");
    assert_eq!(parse_source(p.to_str().unwrap(), None, None).unwrap(), Source::Local(canon.clone()));
    assert_eq!(parse_source("~/skills/x", None, Some(t.path())).unwrap(), Source::Local(canon));
    assert!(matches!(parse_source("./definitely-not-here", None, None), Err(SourceError::NotFound(_))));
}

#[test]
fn github_shorthand_and_urls() {
    assert_eq!(parse_source("anthropics/skills", None, None).unwrap(), git("https://github.com/anthropics/skills", None, None));
    assert_eq!(
        parse_source("anthropics/skills#skills/pdf", None, None).unwrap(),
        git("https://github.com/anthropics/skills", None, Some("skills/pdf"))
    );
    assert_eq!(
        parse_source("https://github.com/anthropics/skills", Some("v2"), None).unwrap(),
        git("https://github.com/anthropics/skills", Some("v2"), None)
    );
    assert_eq!(
        parse_source("https://github.com/anthropics/skills/tree/main/skills/pdf", None, None).unwrap(),
        git("https://github.com/anthropics/skills", Some("main"), Some("skills/pdf"))
    );
    assert_eq!(
        parse_source("https://github.com/o/r/blob/v1/skills/pdf/SKILL.md", None, None).unwrap(),
        git("https://github.com/o/r", Some("v1"), Some("skills/pdf"))
    );
    assert_eq!(
        parse_source("https://gitlab.example.com/team/skills.git#tools/lint", None, None).unwrap(),
        git("https://gitlab.example.com/team/skills.git", None, Some("tools/lint"))
    );
}

#[test]
fn refusals() {
    assert!(matches!(parse_source("git@github.com:o/r.git", None, None), Err(SourceError::UnsupportedScheme(_))));
    assert!(matches!(parse_source("ssh://github.com/o/r", None, None), Err(SourceError::UnsupportedScheme(_))));
    assert!(matches!(parse_source("file:///tmp/r", None, None), Err(SourceError::UnsupportedScheme(_))));
    assert!(matches!(parse_source("http://github.com/o/r", None, None), Err(SourceError::UnsupportedScheme(_))));
    assert!(matches!(parse_source("o/r#../escape", None, None), Err(SourceError::BadPath(_))));
    assert!(matches!(parse_source("o/r#/abs", None, None), Err(SourceError::BadPath(_))));
    assert!(matches!(
        parse_source("https://github.com/o/r/tree/main/x", Some("dev"), None),
        Err(SourceError::RefConflict { .. })
    ));
    assert_eq!(
        parse_source("https://github.com/o/r/tree/main/x", Some("main"), None).unwrap(),
        git("https://github.com/o/r", Some("main"), Some("x"))
    );
    assert!(matches!(parse_source("just-a-word", None, None), Err(SourceError::Unrecognized(_))));
}
```

- [ ] **Step 2: Run it and watch it fail**

Run: `cargo test -p rupu-skills --test it source::`
Expected: compile error, `source` module not found.

- [ ] **Step 3: Implement `source.rs`**

```rust
//! What `rupu skill install` accepts (spec §7.1): a local directory, a
//! GitHub `owner/repo`, a GitHub tree or blob URL, or any https git URL
//! with an optional `#<path>`.

use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Local(PathBuf),
    Git(GitSource),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitSource {
    pub url: String,
    pub git_ref: Option<String>,
    pub path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SourceError {
    #[error("no such directory: {0}")]
    NotFound(String),
    #[error("only https:// git URLs are fetched ({0}); for a local checkout pass its directory")]
    UnsupportedScheme(String),
    #[error("#{0}: the path must be relative and must not contain '..'")]
    BadPath(String),
    #[error("the URL names ref {url_ref:?} but --ref says {flag_ref:?}")]
    RefConflict { url_ref: String, flag_ref: String },
    #[error("{0:?} is not a directory, owner/repo, or https git URL")]
    Unrecognized(String),
    #[error("fetch failed: {0}")]
    Clone(String),
    #[error("#{0} does not exist in the repository")]
    PathMissing(String),
}

pub fn parse_source(input: &str, git_ref: Option<&str>, home: Option<&Path>) -> Result<Source, SourceError> {
    let expanded = match (input.strip_prefix("~/"), home) {
        (Some(rest), Some(h)) => h.join(rest),
        _ => PathBuf::from(input),
    };
    if expanded.is_dir() {
        let canon = expanded.canonicalize().map_err(|_| SourceError::NotFound(input.to_owned()))?;
        return Ok(Source::Local(canon));
    }
    if input.starts_with('.') || input.starts_with('/') || input.starts_with('~') {
        return Err(SourceError::NotFound(input.to_owned()));
    }
    if input.starts_with("git@") {
        return Err(SourceError::UnsupportedScheme(input.to_owned()));
    }
    let (base, hash_path) = match input.split_once('#') {
        Some((b, p)) => (b, Some(check_path(p)?)),
        None => (input, None),
    };
    let flag_ref = git_ref.map(str::to_owned);
    if let Some((scheme, _)) = base.split_once("://") {
        if scheme != "https" {
            return Err(SourceError::UnsupportedScheme(input.to_owned()));
        }
        if let Some(tree) = github_tree(base)? {
            let (url, url_ref, path) = tree;
            if let Some(f) = &flag_ref {
                if *f != url_ref {
                    return Err(SourceError::RefConflict { url_ref, flag_ref: f.clone() });
                }
            }
            return Ok(Source::Git(GitSource { url, git_ref: Some(url_ref), path: hash_path.or(path) }));
        }
        return Ok(Source::Git(GitSource { url: base.trim_end_matches('/').to_owned(), git_ref: flag_ref, path: hash_path }));
    }
    let parts: Vec<&str> = base.split('/').collect();
    let ok = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if parts.len() == 2 && parts.iter().all(|p| ok(p)) {
        return Ok(Source::Git(GitSource {
            url: format!("https://github.com/{}/{}", parts[0], parts[1]),
            git_ref: flag_ref,
            path: hash_path,
        }));
    }
    Err(SourceError::Unrecognized(input.to_owned()))
}

fn check_path(p: &str) -> Result<String, SourceError> {
    let trimmed = p.trim_end_matches('/');
    let path = Path::new(trimmed);
    if trimmed.is_empty() || p.starts_with('/') || path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(SourceError::BadPath(p.to_owned()));
    }
    Ok(trimmed.to_owned())
}

/// `https://github.com/o/r/tree/<ref>/<path>` or `.../blob/<ref>/<path>/SKILL.md`
/// → (repo URL, ref, path). The ref is one segment; a ref with `/` needs
/// `--ref` and `#<path>`.
fn github_tree(url: &str) -> Result<Option<(String, String, Option<String>)>, SourceError> {
    let Some(rest) = url.strip_prefix("https://github.com/") else {
        return Ok(None);
    };
    let segs: Vec<&str> = rest.trim_end_matches('/').split('/').collect();
    if segs.len() < 4 || !matches!(segs[2], "tree" | "blob") {
        return Ok(None);
    }
    let repo = format!("https://github.com/{}/{}", segs[0], segs[1]);
    let mut path_segs: Vec<&str> = segs[4..].to_vec();
    if segs[2] == "blob" && path_segs.last() == Some(&"SKILL.md") {
        path_segs.pop();
    }
    let path = if path_segs.is_empty() { None } else { Some(check_path(&path_segs.join("/"))?) };
    Ok(Some((repo, segs[3].to_owned(), path)))
}

/// A fetched source tree: `root` is the repo root, or `<path>` inside it.
pub struct FetchedTree {
    pub root: PathBuf,
    pub commit: Option<String>,
    _guard: Option<tempfile::TempDir>,
}

impl FetchedTree {
    pub fn local(root: PathBuf) -> Self {
        Self { root, commit: None, _guard: None }
    }

    pub fn with_guard(root: PathBuf, commit: Option<String>, guard: tempfile::TempDir) -> Self {
        Self { root, commit, _guard: Some(guard) }
    }
}

/// How a git source is fetched. `git::GitFetcher` is the production one;
/// tests pass their own.
pub trait Fetch {
    fn fetch(&self, src: &GitSource) -> Result<FetchedTree, SourceError>;
}
```

`install.rs`: add to `InstallError`:

```rust
    #[error(transparent)]
    Source(#[from] crate::source::SourceError),
```

- [ ] **Step 4: Implement `git.rs`** (behind `#[cfg(feature = "git")]` in `lib.rs`), with its unit tests

```rust
//! Fetch a git source with libgit2: no hooks, no submodules, https only.
//! The default branch is a shallow clone; a ref is a full clone checked
//! out detached at that ref.

use crate::source::{Fetch, FetchedTree, GitSource, SourceError};
use std::path::Path;

pub struct GitFetcher;

impl Fetch for GitFetcher {
    fn fetch(&self, src: &GitSource) -> Result<FetchedTree, SourceError> {
        if !src.url.starts_with("https://") {
            return Err(SourceError::UnsupportedScheme(src.url.clone()));
        }
        fetch_any(src)
    }
}

/// The fetch itself, without the scheme check (unit tests clone `file://`).
pub(crate) fn fetch_any(src: &GitSource) -> Result<FetchedTree, SourceError> {
    let guard = tempfile::tempdir().map_err(|e| SourceError::Clone(e.to_string()))?;
    let checkout = guard.path().join(repo_dir_name(&src.url));
    let commit = clone_at(&src.url, &checkout, src.git_ref.as_deref()).map_err(|e| SourceError::Clone(e.message().to_owned()))?;
    let root = match &src.path {
        Some(p) => {
            let r = checkout.join(p);
            if !r.is_dir() {
                return Err(SourceError::PathMissing(p.clone()));
            }
            r
        }
        None => checkout,
    };
    Ok(FetchedTree::with_guard(root, Some(commit), guard))
}

/// The clone's directory name: the repo's last URL segment, so a repo whose
/// root is a skill keeps a meaningful name in `--list` output.
fn repo_dir_name(url: &str) -> String {
    let last = url.trim_end_matches('/').rsplit('/').next().unwrap_or("source");
    let name = last.trim_end_matches(".git");
    if name.is_empty() || name.starts_with('.') {
        "source".into()
    } else {
        name.into()
    }
}

/// Clone `url` into `dest` and return the checked-out commit id.
pub(crate) fn clone_at(url: &str, dest: &Path, git_ref: Option<&str>) -> Result<String, git2::Error> {
    let mut fo = git2::FetchOptions::new();
    if git_ref.is_none() && url.starts_with("https://") {
        fo.depth(1);
    }
    let repo = git2::build::RepoBuilder::new().fetch_options(fo).clone(url, dest)?;
    if let Some(r) = git_ref {
        let obj = [r.to_owned(), format!("origin/{r}"), format!("refs/tags/{r}")]
            .iter()
            .find_map(|spec| repo.revparse_single(spec).ok())
            .ok_or_else(|| git2::Error::from_str(&format!("ref {r:?} not found")))?;
        let commit = obj.peel_to_commit()?;
        repo.checkout_tree(commit.as_object(), Some(git2::build::CheckoutBuilder::new().force()))?;
        repo.set_head_detached(commit.id())?;
    }
    let head = repo.head()?.peel_to_commit()?;
    Ok(head.id().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit_all(repo: &git2::Repository, msg: &str) -> git2::Oid {
        let mut idx = repo.index().unwrap();
        idx.add_all(["*"], git2::IndexAddOption::DEFAULT, None).unwrap();
        idx.write().unwrap();
        let tree = repo.find_tree(idx.write_tree().unwrap()).unwrap();
        let sig = git2::Signature::now("t", "t@example.invalid").unwrap();
        let parents: Vec<git2::Commit> = repo.head().ok().and_then(|h| h.peel_to_commit().ok()).into_iter().collect();
        let refs: Vec<&git2::Commit> = parents.iter().collect();
        repo.commit(Some("HEAD"), &sig, &sig, msg, &tree, &refs).unwrap()
    }

    fn origin() -> (tempfile::TempDir, git2::Oid, git2::Oid) {
        let dir = tempfile::tempdir().unwrap();
        let repo = git2::Repository::init(dir.path()).unwrap();
        let skill = dir.path().join("skills/s");
        std::fs::create_dir_all(&skill).unwrap();
        std::fs::write(skill.join("SKILL.md"), "---\nname: s\ndescription: first\n---\n").unwrap();
        let first = commit_all(&repo, "first");
        let obj = repo.find_object(first, None).unwrap();
        repo.tag_lightweight("v1", &obj, false).unwrap();
        std::fs::write(skill.join("SKILL.md"), "---\nname: s\ndescription: second\n---\n").unwrap();
        let second = commit_all(&repo, "second");
        (dir, first, second)
    }

    #[test]
    fn clones_head_and_a_tag() {
        let (o, first, second) = origin();
        let url = format!("file://{}", o.path().display());

        let head = fetch_any(&GitSource { url: url.clone(), git_ref: None, path: Some("skills/s".into()) }).unwrap();
        assert_eq!(head.commit.as_deref(), Some(second.to_string().as_str()));
        assert!(std::fs::read_to_string(head.root.join("SKILL.md")).unwrap().contains("second"));

        let tag = fetch_any(&GitSource { url, git_ref: Some("v1".into()), path: Some("skills/s".into()) }).unwrap();
        assert_eq!(tag.commit.as_deref(), Some(first.to_string().as_str()));
        assert!(std::fs::read_to_string(tag.root.join("SKILL.md")).unwrap().contains("first"));
    }

    #[test]
    fn missing_path_and_scheme() {
        let (o, _, _) = origin();
        let url = format!("file://{}", o.path().display());
        assert!(matches!(
            fetch_any(&GitSource { url: url.clone(), git_ref: None, path: Some("nope".into()) }),
            Err(SourceError::PathMissing(_))
        ));
        assert!(matches!(
            GitFetcher.fetch(&GitSource { url, git_ref: None, path: None }),
            Err(SourceError::UnsupportedScheme(_))
        ));
    }

    #[test]
    fn repo_dir_names() {
        assert_eq!(repo_dir_name("https://github.com/o/report-style"), "report-style");
        assert_eq!(repo_dir_name("https://h/team/skills.git"), "skills");
        assert_eq!(repo_dir_name("https://h/"), "h");
    }
}
```

`clone_at` asks for `depth(1)` only over https: libgit2's local transport has no shallow support, and that is the only transport the unit tests use.

`lib.rs`: add `pub mod source;`, `pub use source::{parse_source, Fetch, FetchedTree, GitSource, Source, SourceError};`, and

```rust
#[cfg(feature = "git")]
pub mod git;
```

- [ ] **Step 5: Run the tests and watch them pass**

Run: `cargo test -p rupu-skills --test it source::`
Expected: 3 passed.
Run: `cargo test -p rupu-skills --features git --lib git::`
Expected: 3 passed.

- [ ] **Step 6: Commit**

```bash
rustfmt crates/rupu-skills/src/source.rs crates/rupu-skills/src/git.rs crates/rupu-skills/src/install.rs crates/rupu-skills/src/lib.rs crates/rupu-skills/tests/it/source.rs crates/rupu-skills/tests/it/main.rs
git add crates/rupu-skills Cargo.lock
git commit -m "feat(skills): parse install sources and fetch git sources"
```

---

### Task 9: Source records, `install_from` and `update`

**Files:**
- Create: `crates/rupu-skills/src/record.rs`, `crates/rupu-skills/src/update.rs`
- Modify: `crates/rupu-skills/src/install.rs` (add `install_from`, `list_source`), `crates/rupu-skills/src/lib.rs`, `crates/rupu-skills/tests/it/main.rs`
- Test: `crates/rupu-skills/tests/it/update.rs`

**Interfaces:**
- Consumes: `Source`, `GitSource`, `Fetch`, `FetchedTree`, `find_skills`, `install_selected`, `install_one`, `source_hash`, `load_skill`.
- Produces:
  - `SourceRecord { version: u32, source: String, url: Option<String>, git_ref: Option<String> /* JSON "ref" */, path: Option<String>, commit: Option<String>, local: Option<PathBuf>, content_hash: String, installed_at: u64 }`, `Serialize + Deserialize + Debug + Clone + PartialEq`.
  - `record::record_path(skills_dir: &Path, name: &str) -> PathBuf` (`<skills_dir>/.sources/<name>.json`), `record::write_record(skills_dir: &Path, name: &str, rec: &SourceRecord) -> std::io::Result<()>`, `record::read_record(skills_dir: &Path, name: &str) -> Result<Option<SourceRecord>, RecordError>`, `record::list_records(skills_dir: &Path) -> Vec<String>`.
  - `InstallSpec<'a> { source: &'a Source, input: &'a str, selection: Selection, skills_dir: &'a Path, layer: Layer, force: bool }`.
  - `install_from(spec: &InstallSpec<'_>, fetcher: &dyn Fetch) -> Result<InstallReport, InstallError>`: fetches, finds, installs, writes a record per installed skill.
  - `list_source(source: &Source, fetcher: &dyn Fetch) -> Result<Vec<Found>, InstallError>`.
  - `UpdateOutcome::{Updated { from: String, to: String }, UpToDate, NoRecord, LocallyModified, NotInstalled, Failed(String)}`, `Debug + PartialEq + Eq`.
  - `update_skills(skills_dir: &Path, layer: Layer, names: &[String], force: bool, fetcher: &dyn Fetch) -> Vec<(String, UpdateOutcome)>`. Empty `names` means every installed skill with a record.

- [ ] **Step 1: Write the failing test** (`crates/rupu-skills/tests/it/update.rs`; add `mod update;`)

```rust
use crate::support::write_skill;
use rupu_skills::install::{install_from, list_source, InstallSpec, Selection};
use rupu_skills::record::read_record;
use rupu_skills::update::{update_skills, UpdateOutcome};
use rupu_skills::{Fetch, FetchedTree, GitSource, Layer, Source, SourceError};
use std::cell::RefCell;
use std::fs;
use std::path::PathBuf;

/// Serves one "repository" from a local directory, with a settable commit.
struct FakeFetch {
    repo: PathBuf,
    commit: RefCell<String>,
    calls: RefCell<usize>,
}

impl Fetch for FakeFetch {
    fn fetch(&self, src: &GitSource) -> Result<FetchedTree, SourceError> {
        *self.calls.borrow_mut() += 1;
        let root = match &src.path {
            Some(p) => self.repo.join(p),
            None => self.repo.clone(),
        };
        Ok(FetchedTree::local_with_commit(root, Some(self.commit.borrow().clone())))
    }
}

fn gh(path: Option<&str>) -> Source {
    Source::Git(GitSource { url: "https://github.com/o/skills".into(), git_ref: None, path: path.map(Into::into) })
}

fn setup() -> (tempfile::TempDir, tempfile::TempDir, FakeFetch) {
    let repo = tempfile::tempdir().unwrap();
    write_skill(&repo.path().join("skills"), "pdf", "pdf", "v1");
    write_skill(&repo.path().join("skills"), "docx", "docx", "v1");
    let dst = tempfile::tempdir().unwrap();
    let f = FakeFetch { repo: repo.path().to_owned(), commit: RefCell::new("c1".into()), calls: RefCell::new(0) };
    (repo, dst, f)
}

#[test]
fn install_from_git_writes_a_record() {
    let (_repo, dst, f) = setup();
    let source = gh(None);
    let report = install_from(
        &InstallSpec { source: &source, input: "o/skills", selection: Selection::Named(vec!["pdf".into()]), skills_dir: dst.path(), layer: Layer::Global, force: false },
        &f,
    )
    .unwrap();
    assert_eq!(report.installed.len(), 1);
    let rec = read_record(dst.path(), "pdf").unwrap().unwrap();
    assert_eq!(rec.source, "o/skills");
    assert_eq!(rec.url.as_deref(), Some("https://github.com/o/skills"));
    assert_eq!(rec.path.as_deref(), Some("skills/pdf"));
    assert_eq!(rec.commit.as_deref(), Some("c1"));
    assert_eq!(rec.content_hash, report.installed[0].content_hash.hex());
    assert!(dst.path().join(".sources/pdf.json").is_file());
}

#[test]
fn list_source_shows_every_skill() {
    let (_repo, _dst, f) = setup();
    let names: Vec<String> = list_source(&gh(None), &f).unwrap().into_iter().map(|x| x.name).collect();
    assert_eq!(names, vec!["docx", "pdf"]);
}

#[test]
fn update_moves_up_to_date_and_respects_local_edits() {
    let (repo, dst, f) = setup();
    let source = gh(None);
    install_from(
        &InstallSpec { source: &source, input: "o/skills", selection: Selection::All, skills_dir: dst.path(), layer: Layer::Global, force: false },
        &f,
    )
    .unwrap();
    *f.calls.borrow_mut() = 0;

    assert_eq!(
        update_skills(dst.path(), Layer::Global, &[], false, &f),
        vec![("docx".into(), UpdateOutcome::UpToDate), ("pdf".into(), UpdateOutcome::UpToDate)]
    );
    assert_eq!(*f.calls.borrow(), 1, "one fetch per (url, ref)");

    write_skill(&repo.path().join("skills"), "pdf", "pdf", "v2");
    *f.commit.borrow_mut() = "c2".into();
    let got = update_skills(dst.path(), Layer::Global, &["pdf".into()], false, &f);
    assert_eq!(got, vec![("pdf".into(), UpdateOutcome::Updated { from: "c1".into(), to: "c2".into() })]);
    let installed = fs::read_to_string(dst.path().join("pdf/SKILL.md")).unwrap();
    assert!(installed.contains("v2"));
    assert_eq!(read_record(dst.path(), "pdf").unwrap().unwrap().commit.as_deref(), Some("c2"));

    fs::write(dst.path().join("docx/scripts/run.sh"), "echo edited\n").unwrap();
    write_skill(&repo.path().join("skills"), "docx", "docx", "v2");
    assert_eq!(
        update_skills(dst.path(), Layer::Global, &["docx".into()], false, &f),
        vec![("docx".into(), UpdateOutcome::LocallyModified)]
    );
    assert!(matches!(
        update_skills(dst.path(), Layer::Global, &["docx".into()], true, &f)[0].1,
        UpdateOutcome::Updated { .. }
    ));
}

#[test]
fn update_reports_unmanaged_and_missing_skills() {
    let (_repo, dst, f) = setup();
    write_skill(dst.path(), "hand-made", "hand-made", "placed by hand");
    assert_eq!(
        update_skills(dst.path(), Layer::Global, &["hand-made".into(), "ghost".into()], false, &f),
        vec![("hand-made".into(), UpdateOutcome::NoRecord), ("ghost".into(), UpdateOutcome::NotInstalled)]
    );
    assert!(update_skills(dst.path(), Layer::Global, &[], false, &f).is_empty(), "--all covers recorded skills only");
}

#[test]
fn local_sources_update_from_their_directory() {
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    let dir = write_skill(src.path(), "notes", "notes", "v1");
    let source = Source::Local(dir.canonicalize().unwrap());
    let none = FakeFetch { repo: PathBuf::new(), commit: RefCell::new(String::new()), calls: RefCell::new(0) };
    install_from(
        &InstallSpec { source: &source, input: dir.to_str().unwrap(), selection: Selection::Default, skills_dir: dst.path(), layer: Layer::Global, force: false },
        &none,
    )
    .unwrap();
    let rec = read_record(dst.path(), "notes").unwrap().unwrap();
    assert_eq!(rec.local, Some(dir.canonicalize().unwrap()));
    assert_eq!(rec.url, None);

    write_skill(src.path(), "notes", "notes", "v2");
    assert!(matches!(update_skills(dst.path(), Layer::Global, &[], false, &none)[0].1, UpdateOutcome::Updated { .. }));
    assert_eq!(*none.calls.borrow(), 0, "a local source never fetches");
}
```

`FetchedTree::local_with_commit` is a test-friendly constructor; add it to `source.rs` in Step 3.

- [ ] **Step 2: Run it and watch it fail**

Run: `cargo test -p rupu-skills --test it update::`
Expected: compile error, `record` / `update` not found.

- [ ] **Step 3: Implement**

`source.rs`, in `impl FetchedTree`:

```rust
    /// A tree already on disk, with the commit it stands for (if any).
    pub fn local_with_commit(root: PathBuf, commit: Option<String>) -> Self {
        Self { root, commit, _guard: None }
    }
```

`crates/rupu-skills/src/record.rs`:

```rust
//! Where an installed skill came from (spec §7.3), kept outside the skill's
//! own directory so it never moves the skill's fingerprints and is never
//! readable through a run's read roots.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const RECORD_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceRecord {
    pub version: u32,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, rename = "ref", skip_serializing_if = "Option::is_none")]
    pub git_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local: Option<PathBuf>,
    pub content_hash: String,
    pub installed_at: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum RecordError {
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
    #[error("{path}: {source}")]
    Json { path: PathBuf, source: serde_json::Error },
}

pub fn record_path(skills_dir: &Path, name: &str) -> PathBuf {
    skills_dir.join(".sources").join(format!("{name}.json"))
}

pub fn write_record(skills_dir: &Path, name: &str, rec: &SourceRecord) -> std::io::Result<()> {
    let path = record_path(skills_dir, name);
    std::fs::create_dir_all(path.parent().expect("record path has a parent"))?;
    let tmp = path.with_extension(format!("json.{}", std::process::id()));
    std::fs::write(&tmp, serde_json::to_vec_pretty(rec).expect("record serializes"))?;
    std::fs::rename(&tmp, &path)
}

pub fn read_record(skills_dir: &Path, name: &str) -> Result<Option<SourceRecord>, RecordError> {
    let path = record_path(skills_dir, name);
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(RecordError::Io { path, source }),
    };
    serde_json::from_slice(&bytes).map(Some).map_err(|source| RecordError::Json { path, source })
}

/// Names of installed skills that have a record. Orphaned records (the
/// skill directory was deleted) are ignored.
pub fn list_records(skills_dir: &Path) -> Vec<String> {
    let Ok(read) = std::fs::read_dir(skills_dir.join(".sources")) else {
        return Vec::new();
    };
    let mut names: Vec<String> = read
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().to_str()?.strip_suffix(".json").map(str::to_owned))
        .filter(|n| skills_dir.join(n).join("SKILL.md").is_file())
        .collect();
    names.sort();
    names
}

pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}
```

Append to `install.rs`:

```rust
pub struct InstallSpec<'a> {
    pub source: &'a crate::source::Source,
    pub input: &'a str,
    pub selection: Selection,
    pub skills_dir: &'a Path,
    pub layer: Layer,
    pub force: bool,
}

fn open(source: &crate::source::Source, fetcher: &dyn crate::source::Fetch) -> Result<crate::source::FetchedTree, InstallError> {
    Ok(match source {
        crate::source::Source::Local(p) => crate::source::FetchedTree::local(p.clone()),
        crate::source::Source::Git(g) => fetcher.fetch(g)?,
    })
}

pub fn list_source(source: &crate::source::Source, fetcher: &dyn crate::source::Fetch) -> Result<Vec<Found>, InstallError> {
    let tree = open(source, fetcher)?;
    let found = find_skills(&tree.root)?;
    if found.is_empty() {
        return Err(InstallError::NoSkill(tree.root.clone()));
    }
    Ok(found)
}

pub fn install_from(spec: &InstallSpec<'_>, fetcher: &dyn crate::source::Fetch) -> Result<InstallReport, InstallError> {
    use crate::record::{now_secs, write_record, SourceRecord, RECORD_VERSION};
    use crate::source::Source;
    let tree = open(spec.source, fetcher)?;
    let found = find_skills(&tree.root)?;
    if found.is_empty() {
        return Err(InstallError::NoSkill(tree.root.clone()));
    }
    let report = install_selected(&found, &spec.selection, spec.skills_dir, spec.layer, spec.force)?;
    for inst in &report.installed {
        let (url, git_ref, path, local) = match spec.source {
            Source::Git(g) => {
                let joined = match (&g.path, inst.rel.as_str()) {
                    (Some(p), "") => Some(p.clone()),
                    (Some(p), r) => Some(format!("{p}/{r}")),
                    (None, "") => None,
                    (None, r) => Some(r.to_owned()),
                };
                (Some(g.url.clone()), g.git_ref.clone(), joined, None)
            }
            Source::Local(root) => {
                let dir = if inst.rel.is_empty() { root.clone() } else { root.join(&inst.rel) };
                (None, None, None, Some(dir))
            }
        };
        let rec = SourceRecord {
            version: RECORD_VERSION,
            source: spec.input.to_owned(),
            url,
            git_ref,
            path,
            commit: tree.commit.clone(),
            local,
            content_hash: inst.content_hash.hex(),
            installed_at: now_secs(),
        };
        write_record(spec.skills_dir, inst.name.as_str(), &rec).map_err(|source| InstallError::Io {
            path: crate::record::record_path(spec.skills_dir, inst.name.as_str()),
            source,
        })?;
    }
    Ok(report)
}
```

`crates/rupu-skills/src/update.rs`:

```rust
//! `rupu skill update` (spec §7.3): re-fetch installed skills from their
//! recorded source. One fetch per (url, ref); a skill whose installed copy
//! no longer matches its record was edited locally and is left alone
//! unless forced.

use crate::install::{find_skills, install_one, source_hash};
use crate::record::{list_records, now_secs, read_record, write_record, SourceRecord};
use crate::skill::{load_skill, Layer};
use crate::source::{Fetch, FetchedTree, GitSource};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateOutcome {
    Updated { from: String, to: String },
    UpToDate,
    NoRecord,
    LocallyModified,
    NotInstalled,
    Failed(String),
}

pub fn update_skills(
    skills_dir: &Path,
    layer: Layer,
    names: &[String],
    force: bool,
    fetcher: &dyn Fetch,
) -> Vec<(String, UpdateOutcome)> {
    let names: Vec<String> = if names.is_empty() { list_records(skills_dir) } else { names.to_vec() };
    let mut trees: HashMap<(String, Option<String>), Result<FetchedTree, String>> = HashMap::new();
    names
        .into_iter()
        .map(|name| {
            let outcome = update_one(skills_dir, layer, &name, force, fetcher, &mut trees)
                .unwrap_or_else(UpdateOutcome::Failed);
            (name, outcome)
        })
        .collect()
}

fn short(s: &str) -> String {
    s.chars().take(12).collect()
}

fn update_one(
    skills_dir: &Path,
    layer: Layer,
    name: &str,
    force: bool,
    fetcher: &dyn Fetch,
    trees: &mut HashMap<(String, Option<String>), Result<FetchedTree, String>>,
) -> Result<UpdateOutcome, String> {
    let installed_dir = skills_dir.join(name);
    if !installed_dir.join("SKILL.md").is_file() {
        return Ok(UpdateOutcome::NotInstalled);
    }
    let Some(rec) = read_record(skills_dir, name).map_err(|e| e.to_string())? else {
        return Ok(UpdateOutcome::NoRecord);
    };
    let installed = load_skill(&installed_dir, layer).map_err(|e| e.to_string())?;
    let current = installed.content_hash().map_err(|e| e.to_string())?.hex();
    if current != rec.content_hash && !force {
        return Ok(UpdateOutcome::LocallyModified);
    }

    let (source_dir, commit) = match (&rec.local, &rec.url) {
        (Some(dir), _) => (dir.clone(), None),
        (None, Some(url)) => {
            let key = (url.clone(), rec.git_ref.clone());
            let tree = trees.entry(key).or_insert_with(|| {
                fetcher
                    .fetch(&GitSource { url: url.clone(), git_ref: rec.git_ref.clone(), path: None })
                    .map_err(|e| e.to_string())
            });
            let tree = tree.as_ref().map_err(Clone::clone)?;
            (locate(&tree.root, rec.path.as_deref(), name)?, tree.commit.clone())
        }
        (None, None) => return Err("source record has neither url nor local".into()),
    };

    let (_, new_hash) = source_hash(&source_dir).map_err(|e| e.to_string())?;
    let new_rec = SourceRecord {
        commit: commit.clone(),
        content_hash: new_hash.hex(),
        installed_at: now_secs(),
        ..rec.clone()
    };
    if new_hash.hex() == current {
        if new_rec.commit != rec.commit {
            write_record(skills_dir, name, &new_rec).map_err(|e| e.to_string())?;
        }
        return Ok(UpdateOutcome::UpToDate);
    }
    install_one(&source_dir, skills_dir, layer, true).map_err(|e| e.to_string())?;
    write_record(skills_dir, name, &new_rec).map_err(|e| e.to_string())?;
    let label = |commit: &Option<String>, hash: &str| short(commit.as_deref().unwrap_or(hash));
    Ok(UpdateOutcome::Updated { from: label(&rec.commit, &rec.content_hash), to: label(&commit, &new_hash.hex()) })
}

/// The skill's directory in a fetched tree: its recorded path, or a search
/// by name when the repository moved it.
fn locate(root: &Path, path: Option<&str>, name: &str) -> Result<PathBuf, String> {
    if let Some(p) = path {
        let dir = root.join(p);
        if dir.join("SKILL.md").is_file() {
            return Ok(dir);
        }
    }
    find_skills(root)
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|f| f.name == name)
        .map(|f| f.path)
        .ok_or_else(|| format!("{name} is no longer in its source"))
}
```

`lib.rs`: add `pub mod record; pub mod update;`.

Note: the `pdf` record's path is `skills/pdf` and the fake fetch receives `path: None`, so `locate` joins `skills/pdf` onto the repo root. That is the production shape: `update` always clones the whole repo and uses the recorded path.

- [ ] **Step 4: Run it and watch it pass**

Run: `cargo test -p rupu-skills --test it update::`
Expected: 5 passed.

- [ ] **Step 5: Commit**

```bash
rustfmt crates/rupu-skills/src/record.rs crates/rupu-skills/src/update.rs crates/rupu-skills/src/install.rs crates/rupu-skills/src/source.rs crates/rupu-skills/src/lib.rs crates/rupu-skills/tests/it/update.rs crates/rupu-skills/tests/it/main.rs
git add crates/rupu-skills Cargo.lock
git commit -m "feat(skills): record install sources and update from them"
```

---

### Task 10: `project_layer_dir` and `rupu skill`

**Files:**
- Modify: `crates/rupu-workspace/src/config_paths.rs` (`project_config_path`, new `project_layer_dir`), `crates/rupu-workspace/src/lib.rs` (re-export)
- Modify: `crates/rupu-cli/Cargo.toml`, `crates/rupu-cli/src/cmd/mod.rs`, `crates/rupu-cli/src/lib.rs` (`Cmd`, dispatch, `ensure_output_format_supported`)
- Create: `crates/rupu-cli/src/cmd/skill.rs`, `docs/skills.md`
- Modify: `CLAUDE.md` (crate list entry for `rupu-skills`)
- Test: `crates/rupu-workspace/tests/it/config_paths.rs`, `crates/rupu-cli/tests/it/cli_skill.rs`

**Interfaces:**
- Consumes: everything `rupu_skills` exports, and `rupu_skills::git::GitFetcher`.
- Produces: `rupu_workspace::project_layer_dir(global: &Path, project_root: Option<&Path>) -> Option<PathBuf>`, which returns the project's `.rupu/` or `None` under R5. Plan 2's assembler and Plan 4's CP use it for skill discovery.

- [ ] **Step 1: Write the failing workspace test** (append to `crates/rupu-workspace/tests/it/config_paths.rs`)

```rust
#[test]
fn project_layer_dir_drops_the_global_dir() {
    let home = tempfile::tempdir().unwrap();
    let global = home.path().join(".rupu");
    std::fs::create_dir_all(&global).unwrap();
    let repo = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(repo.path().join(".rupu")).unwrap();

    assert_eq!(
        rupu_workspace::project_layer_dir(&global, Some(repo.path())),
        Some(repo.path().join(".rupu"))
    );
    assert_eq!(rupu_workspace::project_layer_dir(&global, Some(home.path())), None);
    assert_eq!(rupu_workspace::project_layer_dir(&global, None), None);
}
```

Run: `cargo test -p rupu-workspace --test it config_paths::project_layer_dir`
Expected: compile error, `project_layer_dir` not found.

- [ ] **Step 2: Implement it, reusing it in `project_config_path`**

In `crates/rupu-workspace/src/config_paths.rs`, replace `project_config_path` with:

```rust
/// The project layer's `.rupu/` for `project_root`, or `None` when there is
/// no project root or its `.rupu/` IS the global dir (R5).
///
/// `project_root_for` treats `~/.rupu` as a project marker, so a repo
/// without its own `.rupu/` resolves to `$HOME`, whose "project layer" is
/// the global dir itself. Loading it a second time as the project layer let
/// global values outrank the customer layer (project beats customer), so the
/// layer is dropped there. Paths are compared canonicalized, or raw when
/// either side cannot be canonicalized.
pub fn project_layer_dir(global: &Path, project_root: Option<&Path>) -> Option<PathBuf> {
    let project_dir = project_root?.join(".rupu");
    let same = match (project_dir.canonicalize(), global.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => project_dir == global,
    };
    (!same).then_some(project_dir)
}

fn project_config_path(global: &Path, project_root: Option<&Path>) -> Option<PathBuf> {
    project_layer_dir(global, project_root).map(|d| d.join("config.toml"))
}
```

Add `project_layer_dir` to the `pub use config_paths::{...}` list in `crates/rupu-workspace/src/lib.rs`.

Run: `cargo test -p rupu-workspace --test it config_paths::`
Expected: all pass, the existing R5 tests included.

- [ ] **Step 3: Write the failing CLI test** (`crates/rupu-cli/tests/it/cli_skill.rs`; add `mod cli_skill;` to `crates/rupu-cli/tests/it/main.rs` in alphabetical position)

```rust
//! `rupu skill`: install, list, show, validate and update against an
//! isolated RUPU_HOME and project, with local-directory sources (no
//! network).

use assert_cmd::Command;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

fn skill_src(dir: &Path, name: &str, description: &str) -> PathBuf {
    let root = dir.join(name);
    fs::create_dir_all(root.join("scripts")).unwrap();
    fs::write(root.join("SKILL.md"), format!("---\nname: {name}\ndescription: {description}\n---\n\nBody.\n")).unwrap();
    fs::write(root.join("scripts/run.sh"), "echo ok\n").unwrap();
    root
}

fn rupu(home: &Path, cwd: &Path, args: &[&str]) -> std::process::Output {
    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", home)
        .env_remove("RUPU_LOG")
        .current_dir(cwd)
        .args(args)
        .output()
        .unwrap()
}

fn ok(out: &std::process::Output) -> String {
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn install_list_show_validate() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    fs::create_dir_all(project.path().join(".rupu")).unwrap();
    let src = tempfile::tempdir().unwrap();
    let pdf = skill_src(src.path(), "pdf-processing", "PDF things.");
    let review = skill_src(src.path(), "code-review", "Review a diff.");

    let out = ok(&rupu(home.path(), project.path(), &["skill", "install", pdf.to_str().unwrap()]));
    assert!(out.contains("installed pdf-processing (global)"), "{out}");
    ok(&rupu(home.path(), project.path(), &["skill", "install", "--project", review.to_str().unwrap()]));
    assert!(project.path().join(".rupu/skills/code-review/SKILL.md").is_file());

    let again = rupu(home.path(), project.path(), &["skill", "install", pdf.to_str().unwrap()]);
    assert!(!again.status.success());
    assert!(String::from_utf8_lossy(&again.stderr).contains("--force"));

    let json: Value = serde_json::from_str(&ok(&rupu(home.path(), project.path(), &["--format", "json", "skill", "list"]))).unwrap();
    let rows: Vec<(String, String, String)> = json["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| (r["name"].as_str().unwrap().into(), r["layer"].as_str().unwrap().into(), r["status"].as_str().unwrap().into()))
        .collect();
    assert_eq!(
        rows,
        vec![
            ("code-review".into(), "project".into(), "ok".into()),
            ("pdf-processing".into(), "global".into(), "ok".into()),
        ]
    );
    assert_eq!(json["rows"][1]["source"].as_str(), Some(pdf.to_str().unwrap()));

    let show = ok(&rupu(home.path(), project.path(), &["skill", "show", "pdf-processing"]));
    assert!(show.contains("scripts/run.sh"), "{show}");
    assert!(show.contains("PDF things."), "{show}");
    assert!(show.contains("source:"), "{show}");

    ok(&rupu(home.path(), project.path(), &["skill", "validate", "--all"]));
}

#[test]
fn a_collection_needs_a_choice() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    skill_src(&repo.path().join("skills"), "pdf", "PDF.");
    skill_src(&repo.path().join("skills"), "docx", "Word.");
    let repo_arg = repo.path().to_str().unwrap();

    let listed = ok(&rupu(home.path(), cwd.path(), &["skill", "install", "--list", repo_arg]));
    assert!(listed.contains("pdf") && listed.contains("docx"), "{listed}");
    assert!(!home.path().join("skills/pdf").exists());

    let none = rupu(home.path(), cwd.path(), &["skill", "install", repo_arg]);
    assert!(!none.status.success());
    assert!(String::from_utf8_lossy(&none.stderr).contains("--skill"));

    ok(&rupu(home.path(), cwd.path(), &["skill", "install", "--skill", "pdf", repo_arg]));
    assert!(home.path().join("skills/pdf/SKILL.md").is_file());
    assert!(!home.path().join("skills/docx").exists());

    ok(&rupu(home.path(), cwd.path(), &["skill", "install", "--all", "--force", repo_arg]));
    assert!(home.path().join("skills/docx/SKILL.md").is_file());
}

#[test]
fn update_follows_a_local_source() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let src = tempfile::tempdir().unwrap();
    let notes = skill_src(src.path(), "notes", "v1");
    ok(&rupu(home.path(), cwd.path(), &["skill", "install", notes.to_str().unwrap()]));

    let same = ok(&rupu(home.path(), cwd.path(), &["skill", "update", "--all"]));
    assert!(same.contains("notes: up to date"), "{same}");

    skill_src(src.path(), "notes", "v2");
    let moved = ok(&rupu(home.path(), cwd.path(), &["skill", "update", "notes"]));
    assert!(moved.contains("notes: updated"), "{moved}");
    assert!(fs::read_to_string(home.path().join("skills/notes/SKILL.md")).unwrap().contains("v2"));
}

#[test]
fn validate_fails_on_an_invalid_skill() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let bad = home.path().join("skills/bad-one");
    fs::create_dir_all(&bad).unwrap();
    fs::write(bad.join("SKILL.md"), "---\nname: other\ndescription: d\n---\n").unwrap();
    let out = rupu(home.path(), cwd.path(), &["skill", "validate", "--all"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stdout).contains("does not match its directory"));
}

#[test]
fn install_project_outside_a_project_is_refused() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let src = tempfile::tempdir().unwrap();
    let s = skill_src(src.path(), "s", "d");
    let out = rupu(home.path(), cwd.path(), &["skill", "install", "--project", s.to_str().unwrap()]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("not inside a rupu project"));
}
```

Run: `cargo test -p rupu-cli --test it cli_skill::`
Expected: FAIL (`skill` is not a subcommand).

- [ ] **Step 4: Wire the crate into the CLI**

`crates/rupu-cli/Cargo.toml`, next to `rupu-codename = { workspace = true }`:

```toml
rupu-skills = { workspace = true, features = ["git"] }
```

`crates/rupu-cli/src/cmd/mod.rs`: `pub mod skill;` (alphabetical position).

`crates/rupu-cli/src/lib.rs`:
- In `enum Cmd`, after `Fleet`:

```rust
    /// Agent Skills: install, update, list, show and validate SKILL.md skills.
    Skill {
        #[command(subcommand)]
        action: cmd::skill::Action,
    },
```

- In the dispatch `match cli.command`, after the `Cmd::Fleet` arm: `Cmd::Skill { action } => cmd::skill::handle(action, cli.format).await,`
- In `ensure_output_format_supported`, after the `Cmd::Fleet` arm: `Cmd::Skill { action } => cmd::skill::ensure_output_format(action, format),`
- If `owns_live_terminal` (`lib.rs:278`) matches exhaustively, add `Cmd::Skill { .. } => false`. The compiler says whether it is needed.

- [ ] **Step 5: Implement `crates/rupu-cli/src/cmd/skill.rs`**

```rust
//! `rupu skill`: manage Agent Skills in the global (`~/.rupu/skills`) and
//! project (`.rupu/skills`) layers. Thin over `rupu_skills`.

use crate::output::formats::OutputFormat;
use crate::output::report::{self, CollectionOutput};
use clap::Subcommand;
use rupu_skills::git::GitFetcher;
use rupu_skills::install::{install_from, list_source, InstallOutcome, InstallSpec, Selection};
use rupu_skills::record::read_record;
use rupu_skills::tools_map::mapped_tools;
use rupu_skills::update::{update_skills, UpdateOutcome};
use rupu_skills::{parse_source, CatalogEntry, Layer, LayerDirs, SkillCatalog};
use serde::Serialize;
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Subcommand, Debug)]
pub enum Action {
    /// List installed skills in both layers.
    List {
        /// Only the project layer.
        #[arg(long)]
        project: bool,
    },
    /// Show one skill: frontmatter, files, fingerprints and source.
    /// Accepts `global:<name>` / `project:<name>`.
    Show { name: String },
    /// Check skills against the Agent Skills format; exit 1 on any failure.
    Validate {
        name: Option<String>,
        #[arg(long, conflicts_with = "name")]
        all: bool,
    },
    /// Install skills from a directory, `owner/repo`, a GitHub tree URL, or
    /// an https git URL (`#path` narrows to a folder in the repository).
    Install {
        source: String,
        /// Branch, tag or commit to install from.
        #[arg(long = "ref")]
        git_ref: Option<String>,
        /// Install this skill from a source that holds several (repeatable).
        #[arg(long = "skill")]
        skills: Vec<String>,
        /// Install every valid skill in the source.
        #[arg(long, conflicts_with = "skills")]
        all: bool,
        /// List the skills in the source; install nothing.
        #[arg(long)]
        list: bool,
        /// Install into the current project's `.rupu/skills` instead of
        /// `~/.rupu/skills`.
        #[arg(long)]
        project: bool,
        /// Replace installed skills of the same name.
        #[arg(long)]
        force: bool,
    },
    /// Re-fetch installed skills from the source they were installed from.
    Update {
        names: Vec<String>,
        /// Every installed skill with a source record.
        #[arg(long, conflicts_with = "names")]
        all: bool,
        /// The project layer instead of the global one.
        #[arg(long)]
        project: bool,
        /// Replace skills whose installed copy was edited.
        #[arg(long)]
        force: bool,
    },
}

pub fn ensure_output_format(action: &Action, format: OutputFormat) -> anyhow::Result<()> {
    let (command_name, supported) = match action {
        Action::List { .. } => ("skill list", report::TABLE_JSON_CSV),
        _ => ("skill", report::TABLE_ONLY),
    };
    crate::output::formats::ensure_supported(command_name, format, supported)
}

pub async fn handle(action: Action, format: Option<OutputFormat>) -> ExitCode {
    let result = match action {
        Action::List { project } => list(project, format).map(|()| true),
        Action::Show { name } => show(&name).map(|()| true),
        Action::Validate { name, all } => validate(name.as_deref(), all),
        Action::Install { source, git_ref, skills, all, list, project, force } => {
            install(&source, git_ref.as_deref(), skills, all, list, project, force).map(|()| true)
        }
        Action::Update { names, all, project, force } => update(names, all, project, force),
    };
    match result {
        Ok(true) => ExitCode::from(0),
        Ok(false) => ExitCode::from(1),
        Err(e) => crate::output::diag::fail(e),
    }
}

struct Layers {
    global: PathBuf,
    project: Option<PathBuf>,
}

impl Layers {
    fn skills_dir(&self, layer: Layer) -> anyhow::Result<PathBuf> {
        match layer {
            Layer::Global => Ok(self.global.join("skills")),
            Layer::Project => self.project.as_ref().map(|p| p.join("skills")).ok_or_else(|| {
                anyhow::anyhow!(
                    "not inside a rupu project (no `.rupu/` found). Run `rupu init` first, \
                     or omit --project to use ~/.rupu/skills."
                )
            }),
        }
    }
}

fn layers() -> anyhow::Result<Layers> {
    let global = crate::paths::global_dir()?;
    let cwd = std::env::current_dir()?;
    let root = crate::paths::project_root_for(&cwd)?;
    let project = rupu_workspace::project_layer_dir(&global, root.as_deref());
    Ok(Layers { global, project })
}

fn catalog(l: &Layers) -> anyhow::Result<SkillCatalog> {
    Ok(SkillCatalog::discover(&LayerDirs {
        global: Some(l.global.join("skills")),
        project: l.project.as_ref().map(|p| p.join("skills")),
    })?)
}

fn layer_flag(project: bool) -> Layer {
    if project {
        Layer::Project
    } else {
        Layer::Global
    }
}

#[derive(Debug, Clone, Serialize)]
struct SkillRow {
    name: String,
    layer: &'static str,
    status: &'static str,
    description: String,
    source: String,
    error: String,
    path: String,
}

#[derive(Debug, Clone, Serialize)]
struct SkillsReport {
    kind: &'static str,
    version: u8,
    rows: Vec<SkillRow>,
}

struct SkillsOutput {
    report: SkillsReport,
}

impl CollectionOutput for SkillsOutput {
    type JsonReport = SkillsReport;
    type CsvRow = SkillRow;

    fn command_name(&self) -> &'static str {
        "skill list"
    }

    fn json_report(&self) -> &Self::JsonReport {
        &self.report
    }

    fn csv_rows(&self) -> &[Self::CsvRow] {
        &self.report.rows
    }

    fn render_table(&self) -> anyhow::Result<()> {
        let mut table = crate::output::tables::new_table();
        table.set_header(vec!["NAME", "LAYER", "STATUS", "SOURCE", "DESCRIPTION"]);
        for r in &self.report.rows {
            let detail = if r.status == "ok" { &r.description } else { &r.error };
            let detail: String = detail.chars().take(70).collect();
            let source = if r.source.is_empty() { "-".to_owned() } else { r.source.clone() };
            table.add_row(vec![r.name.clone(), r.layer.into(), r.status.into(), source, detail]);
        }
        println!("{table}");
        Ok(())
    }
}

fn skills_dir_of(e: &CatalogEntry) -> PathBuf {
    e.path.parent().map(PathBuf::from).unwrap_or_default()
}

fn row(e: &CatalogEntry) -> SkillRow {
    let (status, description, error) = match &e.skill {
        Ok(s) => ("ok", s.frontmatter.description.clone(), String::new()),
        Err(err) => ("invalid", String::new(), err.to_string()),
    };
    let source = read_record(&skills_dir_of(e), &e.dir_name)
        .ok()
        .flatten()
        .map(|r| r.source)
        .unwrap_or_default();
    SkillRow {
        name: e.dir_name.clone(),
        layer: e.layer.as_str(),
        status,
        description,
        source,
        error,
        path: e.path.display().to_string(),
    }
}

fn list(project_only: bool, format: Option<OutputFormat>) -> anyhow::Result<()> {
    let cat = catalog(&layers()?)?;
    let rows = cat
        .entries()
        .iter()
        .filter(|e| !project_only || e.layer == Layer::Project)
        .map(row)
        .collect();
    report::emit_collection(format, &SkillsOutput { report: SkillsReport { kind: "skills", version: 1, rows } })
}

/// `global:x` / `project:x` pick a layer; a bare name matches both.
fn select<'a>(cat: &'a SkillCatalog, name: &str) -> Vec<&'a CatalogEntry> {
    match name.split_once(':').and_then(|(l, n)| Layer::parse(l).map(|l| (l, n))) {
        Some((layer, n)) => cat.get(layer, n).into_iter().collect(),
        None => cat.named(name).collect(),
    }
}

fn show(name: &str) -> anyhow::Result<()> {
    let cat = catalog(&layers()?)?;
    let found = select(&cat, name);
    anyhow::ensure!(!found.is_empty(), "no installed skill named {name:?}");
    if found.len() > 1 {
        println!("{name} is installed in both layers; grant it as global:{name} or project:{name}.\n");
    }
    for e in found {
        println!("{} ({})", e.dir_name, e.layer.as_str());
        println!("  path:          {}", e.path.display());
        if let Some(rec) = read_record(&skills_dir_of(e), &e.dir_name)? {
            let at = match (&rec.url, &rec.local) {
                (Some(url), _) => format!(
                    "{url}{}{} @ {}",
                    rec.git_ref.as_deref().map(|r| format!(" ref {r}")).unwrap_or_default(),
                    rec.path.as_deref().map(|p| format!(" #{p}")).unwrap_or_default(),
                    rec.commit.as_deref().map(|c| &c[..c.len().min(12)]).unwrap_or("?")
                ),
                (None, Some(dir)) => dir.display().to_string(),
                (None, None) => "?".into(),
            };
            println!("  source:        {} ({at})", rec.source);
        }
        let s = match &e.skill {
            Ok(s) => s,
            Err(err) => {
                println!("  INVALID:       {err}\n");
                continue;
            }
        };
        let fm = &s.frontmatter;
        println!("  description:   {}", fm.description);
        if let Some(l) = &fm.license {
            println!("  license:       {l}");
        }
        if let Some(c) = &fm.compatibility {
            println!("  compatibility: {c}");
        }
        if !fm.allowed_tools.is_empty() {
            println!(
                "  allowed-tools: {} (rupu: {}; informational, never a grant)",
                fm.allowed_tools.join(" "),
                mapped_tools(&fm.allowed_tools).join(", ")
            );
        }
        if fm.disable_model_invocation {
            println!("  model invocation: disabled (preload or operator invocation only)");
        }
        for (k, v) in &fm.metadata {
            println!("  metadata.{k}: {v}");
        }
        println!("  stamp:         {}", s.stamp.short());
        println!("  content:       {}", s.content_hash()?.short());
        println!("  files:");
        for f in s.files() {
            println!("    {f}");
        }
        println!();
    }
    Ok(())
}

fn validate(name: Option<&str>, all: bool) -> anyhow::Result<bool> {
    let cat = catalog(&layers()?)?;
    let entries: Vec<&CatalogEntry> = match name {
        Some(n) => select(&cat, n),
        None if all => cat.entries().iter().collect(),
        None => anyhow::bail!("name a skill or pass --all"),
    };
    anyhow::ensure!(!entries.is_empty() || all, "no installed skill named {:?}", name.unwrap_or_default());
    let mut ok = true;
    for e in entries {
        match &e.skill {
            Ok(_) => println!("ok       {} ({})", e.dir_name, e.layer.as_str()),
            Err(err) => {
                ok = false;
                println!("INVALID  {} ({}): {err}", e.dir_name, e.layer.as_str());
            }
        }
    }
    Ok(ok)
}

fn install(
    input: &str,
    git_ref: Option<&str>,
    skills: Vec<String>,
    all: bool,
    list_only: bool,
    project: bool,
    force: bool,
) -> anyhow::Result<()> {
    let home = dirs::home_dir();
    let source = parse_source(input, git_ref, home.as_deref())?;
    if list_only {
        for f in list_source(&source, &GitFetcher)? {
            let detail = f.error.or(f.description).unwrap_or_default();
            let at = if f.rel.is_empty() { ".".into() } else { f.rel };
            println!("{:<28} {:<36} {}", f.name, at, detail.chars().take(70).collect::<String>());
        }
        return Ok(());
    }
    let l = layers()?;
    let layer = layer_flag(project);
    let skills_dir = l.skills_dir(layer)?;
    let selection = if all {
        Selection::All
    } else if !skills.is_empty() {
        Selection::Named(skills)
    } else {
        Selection::Default
    };
    let report = install_from(
        &InstallSpec { source: &source, input, selection, skills_dir: &skills_dir, layer, force },
        &GitFetcher,
    )?;
    for i in &report.installed {
        let verb = match i.outcome {
            InstallOutcome::Created => "installed",
            InstallOutcome::Replaced => "replaced",
        };
        println!("{verb} {} ({}) at {}", i.name, layer.as_str(), i.path.display());
    }
    for (name, why) in &report.skipped {
        eprintln!("skipped {name}: {why}");
    }
    Ok(())
}

fn update(names: Vec<String>, all: bool, project: bool, force: bool) -> anyhow::Result<bool> {
    anyhow::ensure!(all || !names.is_empty(), "name skills to update or pass --all");
    let l = layers()?;
    let layer = layer_flag(project);
    let skills_dir = l.skills_dir(layer)?;
    let results = update_skills(&skills_dir, layer, &names, force, &GitFetcher);
    if results.is_empty() {
        println!("no installed skills with a source record in the {} layer", layer.as_str());
    }
    let mut ok = true;
    for (name, outcome) in results {
        let line = match outcome {
            UpdateOutcome::Updated { from, to } => format!("updated {from} → {to}"),
            UpdateOutcome::UpToDate => "up to date".into(),
            UpdateOutcome::NoRecord => "no source record (installed by hand); reinstall it to track a source".into(),
            UpdateOutcome::LocallyModified => {
                ok = false;
                "locally modified; pass --force to replace your edits".into()
            }
            UpdateOutcome::NotInstalled => {
                ok = false;
                "not installed".into()
            }
            UpdateOutcome::Failed(e) => {
                ok = false;
                format!("failed: {e}")
            }
        };
        println!("{name}: {line}");
    }
    Ok(ok)
}
```

The CLI test's `contains("notes: updated")` matches the `{name}: updated {from} → {to}` line. If `dirs` is not already a `rupu-cli` dependency, use `crate::paths`' home lookup instead; `paths.rs` already calls `dirs::home_dir()`. If `report::TABLE_JSON_CSV`, `report::TABLE_ONLY` or `crate::output::tables::new_table` are named differently, use the names `cmd/customer.rs` uses; the shapes above are copied from it.

- [ ] **Step 6: Run the CLI tests and watch them pass**

Run: `cargo test -p rupu-cli --test it cli_skill::`
Expected: 5 passed.

Also run `cargo test -p rupu-cli --test it test_layout::`, which checks the new test file mutates no process state. Expected: pass.

- [ ] **Step 7: Write `docs/skills.md`**

Cover, in this order:
- What a skill is (link agentskills.io).
- The two layers, and why `.claude/skills` is not read.
- The `SKILL.md` rules table (copy spec §1.1).
- Installing:
  - the source table from spec §7.1, with `rupu skill install anthropics/skills --list` and `rupu skill install anthropics/skills --skill pdf` as examples;
  - `--ref`, `--all`, `--project` and `--force`;
  - a note that a skill's scripts need their own tools on the machine (and on any remote host an agent runs on).
- `update`: source records, pinned refs, and locally modified skills.
- `list`, `show` and `validate`.
- A closing line saying that granting skills to agents arrives with the `skills:` frontmatter key (Plan 2), linking the spec.

Use an invented example skill (`report-style`) for anything beyond the two `anthropics/skills` commands.

Add to `CLAUDE.md`'s crate list, after `rupu-codename`:

```markdown
- **`rupu-skills`** — Agent Skills (agentskills.io `SKILL.md` directories; spec `docs/superpowers/specs/2026-10-12-rupu-agent-skills-design.md`). Leaf crate (no rupu deps): `SkillName` rules, `parse_skill_md` (unknown keys accepted), `walk_tree` + `Stamp` (stat-only) / `ContentHash` (bytes, on load), `load_skill` / `SkillCatalog` over the global and project `skills/` dirs (both layers kept, no shadowing; project symlinked dirs invalid), the grant grammar (`*` = global only, `project:*`, `global:x` / `project:x`, a bare name in both layers is an error), the prompt renderers whose text is the model-facing contract, `allowed-tools` → core-tool map (never a grant), install (`source::parse_source`: path / `owner/repo` / GitHub tree URL / https `#path` + `--ref`; `install::find_skills` searches up to 6 levels, skipping `.git`/`node_modules`; staged, validated, renamed in), source records in `<skills>/.sources/<name>.json` and `update::update_skills` (one fetch per url+ref; a locally edited copy is left alone without `--force`). The `git` feature adds `git::GitFetcher` (libgit2, https only, no hooks or submodules). The project layer comes from `rupu_workspace::project_layer_dir` (R5). CLI: `rupu skill list|show|validate|install|update`.
```

- [ ] **Step 8: Lint the touched crates**

Run: `cargo clippy -p rupu-skills --all-targets --features git -- -D warnings`
Run: `cargo clippy -p rupu-cli -p rupu-workspace --all-targets -- -D warnings`
Expected: no warnings from touched files. Report pre-existing ones without fixing them.

- [ ] **Step 9: Commit**

```bash
rustfmt crates/rupu-cli/src/cmd/skill.rs crates/rupu-cli/tests/it/cli_skill.rs crates/rupu-workspace/src/config_paths.rs
git add crates/rupu-workspace crates/rupu-cli docs/skills.md CLAUDE.md Cargo.lock
git commit -m "feat(cli): add rupu skill list, show, validate, install and update"
```

---

## Done when

- `cargo test -p rupu-skills --test it` and `cargo test -p rupu-skills --features git --lib` pass.
- `cargo test -p rupu-workspace --test it config_paths::` and `cargo test -p rupu-cli --test it cli_skill::` pass.
- By hand, with a scratch `RUPU_HOME`: `rupu skill install anthropics/skills --list` lists the collection; `rupu skill install anthropics/skills --skill pdf` installs one skill, and its scripts keep their exec bit (`ls -l` on the installed copy); the browser URL `https://github.com/anthropics/skills/tree/main/skills/pdf` resolves to the same skill; `rupu skill update --all` reports it up to date; `rupu skill validate --all` exits 0.
- Nothing in this plan touches agent loading, the assembler or any run. Those start in Plan 2.

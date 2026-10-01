# Engagement profiles — Plan 1 (contract) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Land the domain-agnostic asset + engagement-profile contract in `rupu-coverage` and `rupu-findings-report` — the typed primitive vocabulary, the profile data package + loader + registry, and disasm/hexdump rendering — as a self-contained, unit-tested library layer with no runtime wiring.

**Architecture:** Option A′ from the spec: the core owns a small fixed primitive vocabulary (`Coordinate`, `EvidenceBlock`, `Predicate`); an engagement profile is a data package (TOML for `binary`, a native value for `code`) composed from those primitives. Assets are profile-namespaced (`binary:function`); findings will route to a validating profile by asset-kind namespace. This plan is additive and gated: nothing here changes existing `code` behavior — every new field is `#[serde(default)]`, and the registry ships `code` (native, unchanged) + `binary` (data).

**Tech Stack:** Rust 2021, `serde`, `serde_json`, `toml`, `sha2`, `thiserror`. Tests are `#[test]` + `serde_json`.

**Spec:** `docs/superpowers/specs/2026-09-30-rupu-engagement-profiles-asset-model-design.md`

## Global Constraints

- MSRV pinned in `rust-toolchain.toml`; `#![deny(clippy::all)]` and `#![forbid(unsafe_code)]` hold workspace-wide — every new module inherits them.
- Workspace deps only: versions live in root `Cargo.toml`, never in a crate `Cargo.toml`. `toml`, `sha2`, `serde`, `serde_json` are used here — confirm each is in `[workspace.dependencies]` and reference it as `foo = { workspace = true }` in `crates/rupu-coverage/Cargo.toml`; add to the root if missing.
- Additive & backwards-compatible: every new `FindingReport`/`EvidenceClaim` field is `#[serde(default)]` (and `skip_serializing_if` where empty), so legacy records deserialize unchanged. Do not alter or reorder existing fields.
- The finding-report schema is a lockstep contract: `crates/rupu-coverage/schema/finding_report.schema.json` ↔ `validate_report` ↔ `tests/report_schema_lockstep.rs`. Any model change that the canonical schema describes must update the JSON in the same task, and `cargo test -p rupu-coverage report_schema_lockstep` must stay green.
- Asset kinds are profile-namespaced at registry level (`<profile>:<kind>`, e.g. `binary:function`); inside a profile package a kind id is bare (`function`).
- Do not invent examples from real assessments (public repo). All fixtures are synthetic.
- Run `cargo test -p rupu-coverage` and `cargo clippy -p rupu-coverage --all-targets` (plus `-p rupu-findings-report` for Task 6) green before each commit.

---

## File Structure

**`crates/rupu-coverage/`** (new + modified)
- Create `src/asset/mod.rs` — module root, re-exports.
- Create `src/asset/coordinate.rs` — `Coordinate`, `Proto`, `Locator`.
- Create `src/asset/types.rs` — `Asset`, `AssetId`, kind-namespace helpers.
- Create `src/asset/graph.rs` — in-memory `AssetGraph` (tree build + depth update).
- Create `src/profile/mod.rs` — module root; `EngagementProfile`, `AssetKindDef`, `CoverageSpec`, `Bundle`, `code_profile()`.
- Create `src/profile/package.rs` — TOML parse into `EngagementProfile`.
- Create `src/profile/predicate.rs` — `Predicate`, `CompletenessCheck`, evaluation + scoring.
- Create `src/profile/loader.rs` — embedded + filesystem discovery, composite `includes` expansion.
- Create `src/profile/registry.rs` — `ProfileRegistry`, active-set, `profile_for_kind`, resolution + narrow-only.
- Create `src/profile/builtin/binary.toml` — the embedded `binary` profile.
- Modify `src/lib.rs` — add `pub mod asset;`, `pub mod profile;`, re-exports.
- Modify `src/report/types.rs` — add `Classification`, `EvidenceBlock`; add `classifications` and per-claim `blocks` fields.
- Modify `schema/finding_report.schema.json` — describe the two additive fields.

**`crates/rupu-findings-report/`** (modified)
- Modify `src/blocks.rs` — map `EvidenceBlock::{Disasm,Hexdump,Table,...}` into the existing `Block` model.

---

## Task 1: Coordinate + Locator primitives

**Files:**
- Create: `crates/rupu-coverage/src/asset/coordinate.rs`
- Create: `crates/rupu-coverage/src/asset/mod.rs`
- Modify: `crates/rupu-coverage/src/lib.rs` (add `pub mod asset;` + re-export)
- Test: inline `#[cfg(test)]` in `coordinate.rs`

**Interfaces:**
- Produces: `Coordinate` (enum), `Proto` (enum `Tcp|Udp`), `Locator(pub Vec<Coordinate>)`, `Coordinate::tag(&self) -> &'static str`, `Locator::has(&self, tag: &str) -> bool`.

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_and_has_and_roundtrip() {
        let loc = Locator(vec![
            Coordinate::Host("10.0.0.1".into()),
            Coordinate::Port { number: 443, proto: Proto::Tcp },
        ]);
        assert!(loc.has("host") && loc.has("port") && !loc.has("url"));
        assert_eq!(Coordinate::Address(0x401000).tag(), "address");
        let j = serde_json::to_string(&loc).unwrap();
        assert_eq!(serde_json::from_str::<Locator>(&j).unwrap(), loc);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rupu-coverage coordinate::tests -- --nocapture`
Expected: FAIL — `asset` module / `Coordinate` not found.

- [ ] **Step 3: Write minimal implementation**

`src/asset/coordinate.rs`:

```rust
//! The typed locator primitives the core understands. Adding a variant is the
//! only per-kind reason to touch core code — rare, shared by every profile.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Proto {
    Tcp,
    Udp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Coordinate {
    // static / code / binary
    Path(String),
    LineRange { start: u32, end: u32 },
    Symbol(String),
    Commit(String),
    Sha256(String),
    Offset(u64),
    Address(u64),
    // network
    Host(String),
    Port { number: u16, proto: Proto },
    Url(String),
    // web / appsec
    HttpRoute { method: String, path: String },
    Param(String),
    // cloud / saas / k8s
    ResourceId { scheme: String, id: String },
}

impl Coordinate {
    pub fn tag(&self) -> &'static str {
        match self {
            Coordinate::Path(_) => "path",
            Coordinate::LineRange { .. } => "line_range",
            Coordinate::Symbol(_) => "symbol",
            Coordinate::Commit(_) => "commit",
            Coordinate::Sha256(_) => "sha256",
            Coordinate::Offset(_) => "offset",
            Coordinate::Address(_) => "address",
            Coordinate::Host(_) => "host",
            Coordinate::Port { .. } => "port",
            Coordinate::Url(_) => "url",
            Coordinate::HttpRoute { .. } => "http_route",
            Coordinate::Param(_) => "param",
            Coordinate::ResourceId { .. } => "resource_id",
        }
    }

    /// Every tag a profile may name in a kind's `coordinates` list.
    pub fn known_tag(tag: &str) -> bool {
        matches!(
            tag,
            "path" | "line_range" | "symbol" | "commit" | "sha256" | "offset"
                | "address" | "host" | "port" | "url" | "http_route" | "param"
                | "resource_id"
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Locator(pub Vec<Coordinate>);

impl Locator {
    pub fn has(&self, tag: &str) -> bool {
        self.0.iter().any(|c| c.tag() == tag)
    }
}
```

`src/asset/mod.rs`:

```rust
//! Asset model: the profile-namespaced subjects a finding is about.
pub mod coordinate;
pub mod graph;
pub mod types;

pub use coordinate::{Coordinate, Locator, Proto};
pub use graph::AssetGraph;
pub use types::{Asset, AssetId};
```

In `src/lib.rs`, add after `pub mod audit;`: `pub mod asset;` and after the existing re-exports:

```rust
pub use asset::{Asset, AssetGraph, AssetId, Coordinate, Locator, Proto};
```

(`graph`/`types` are added in Tasks 2–3; if compiling Task 1 alone, temporarily comment the `graph`/`types` lines in `mod.rs` and the corresponding re-exports, and restore them in those tasks.)

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p rupu-coverage coordinate::tests`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-coverage/src/asset crates/rupu-coverage/src/lib.rs
git commit -m "feat(coverage): Coordinate + Locator primitive vocabulary"
```

---

## Task 2: Asset + AssetId + kind namespace

**Files:**
- Create: `crates/rupu-coverage/src/asset/types.rs`
- Test: inline in `types.rs`

**Interfaces:**
- Consumes: `Locator` (Task 1).
- Produces: `AssetId(pub String)`; `Asset { id, kind, parent: Option<AssetId>, locator, label, depth: Option<String>, attributes }`; `Asset::new(kind, locator, label) -> Asset` (derives `id`); `profile_of(kind: &str) -> &str` (namespace before `:`, or the whole string if unqualified).

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::asset::{Coordinate, Locator};

    #[test]
    fn id_is_stable_and_namespace_parses() {
        let loc = Locator(vec![Coordinate::Address(0x401000), Coordinate::Symbol("main".into())]);
        let a = Asset::new("binary:function", loc.clone(), "main @ 0x401000");
        let b = Asset::new("binary:function", loc, "main @ 0x401000");
        assert_eq!(a.id, b.id, "same kind+locator ⇒ same id");
        assert_eq!(profile_of("binary:function"), "binary");
        assert_eq!(profile_of("code"), "code");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rupu-coverage asset::types`
Expected: FAIL — `Asset` not found.

- [ ] **Step 3: Write minimal implementation**

`src/asset/types.rs`:

```rust
use crate::asset::Locator;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AssetId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Asset {
    pub id: AssetId,
    /// Profile-namespaced kind id, e.g. `binary:function`.
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<AssetId>,
    pub locator: Locator,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depth: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub attributes: BTreeMap<String, serde_json::Value>,
}

impl Asset {
    pub fn new(kind: impl Into<String>, locator: Locator, label: impl Into<String>) -> Self {
        let kind = kind.into();
        let mut h = Sha256::new();
        h.update(kind.as_bytes());
        h.update([0u8]);
        // Locator serialization is deterministic (ordered Vec) ⇒ stable id.
        h.update(serde_json::to_vec(&locator).expect("locator serializes"));
        let id = AssetId(format!("ast_{:x}", h.finalize())[..20].to_string());
        Asset { id, kind, parent: None, locator, label: label.into(), depth: None, attributes: BTreeMap::new() }
    }
}

/// The owning profile of a namespaced kind (`binary:function` ⇒ `binary`).
pub fn profile_of(kind: &str) -> &str {
    kind.split_once(':').map(|(p, _)| p).unwrap_or(kind)
}
```

Note: `format!("ast_{:x}", ...)[..20]` — take the hex string then truncate; write it as a two-step `let hex = format!("{:x}", h.finalize()); AssetId(format!("ast_{}", &hex[..16]))` to avoid slicing a temporary. Use:

```rust
let hex = format!("{:x}", h.finalize());
let id = AssetId(format!("ast_{}", &hex[..16]));
```

Restore the `pub mod types;` / re-export lines in `asset/mod.rs` if they were commented in Task 1.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p rupu-coverage asset::types`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-coverage/src/asset
git commit -m "feat(coverage): Asset, AssetId, profile-namespaced kinds"
```

---

## Task 3: AssetGraph (tree + depth)

**Files:**
- Create: `crates/rupu-coverage/src/asset/graph.rs`
- Test: inline in `graph.rs`

**Interfaces:**
- Consumes: `Asset`, `AssetId` (Task 2).
- Produces: `AssetGraph` with `insert(&mut self, Asset)`, `roots(&self) -> Vec<&Asset>`, `children(&self, &AssetId) -> Vec<&Asset>`, `set_depth(&mut self, &AssetId, depth: impl Into<String>) -> bool`, `get(&self, &AssetId) -> Option<&Asset>`. Serializable (serde) so Plan 2 can persist it.

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::asset::{Asset, Coordinate, Locator};

    #[test]
    fn builds_tree_and_updates_depth() {
        let root = Asset::new("binary:binary", Locator(vec![Coordinate::Sha256("ab".into())]), "blob");
        let mut child = Asset::new("binary:function", Locator(vec![Coordinate::Address(0x1000)]), "f");
        child.parent = Some(root.id.clone());
        let (rid, cid) = (root.id.clone(), child.id.clone());

        let mut g = AssetGraph::default();
        g.insert(root);
        g.insert(child);

        assert_eq!(g.roots().len(), 1);
        assert_eq!(g.children(&rid).len(), 1);
        assert!(g.set_depth(&cid, "analyzed"));
        assert_eq!(g.get(&cid).unwrap().depth.as_deref(), Some("analyzed"));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rupu-coverage asset::graph`
Expected: FAIL — `AssetGraph` not found.

- [ ] **Step 3: Write minimal implementation**

`src/asset/graph.rs`:

```rust
use crate::asset::{Asset, AssetId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetGraph {
    /// Insertion-preserving map keyed by id (last write wins on re-insert).
    nodes: BTreeMap<String, Asset>,
    order: Vec<String>,
}

impl AssetGraph {
    pub fn insert(&mut self, asset: Asset) {
        if !self.nodes.contains_key(&asset.id.0) {
            self.order.push(asset.id.0.clone());
        }
        self.nodes.insert(asset.id.0.clone(), asset);
    }

    pub fn get(&self, id: &AssetId) -> Option<&Asset> {
        self.nodes.get(&id.0)
    }

    pub fn roots(&self) -> Vec<&Asset> {
        self.order
            .iter()
            .filter_map(|k| self.nodes.get(k))
            .filter(|a| a.parent.is_none())
            .collect()
    }

    pub fn children(&self, parent: &AssetId) -> Vec<&Asset> {
        self.order
            .iter()
            .filter_map(|k| self.nodes.get(k))
            .filter(|a| a.parent.as_ref() == Some(parent))
            .collect()
    }

    pub fn set_depth(&mut self, id: &AssetId, depth: impl Into<String>) -> bool {
        match self.nodes.get_mut(&id.0) {
            Some(a) => {
                a.depth = Some(depth.into());
                true
            }
            None => false,
        }
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p rupu-coverage asset::graph`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-coverage/src/asset/graph.rs crates/rupu-coverage/src/asset/mod.rs
git commit -m "feat(coverage): in-memory AssetGraph with parent links and depth"
```

---

## Task 4: Classification on FindingReport

**Files:**
- Modify: `crates/rupu-coverage/src/report/types.rs` (add `Classification`, `classifications` field, cwe fold accessor)
- Modify: `crates/rupu-coverage/schema/finding_report.schema.json` (describe `classifications`)
- Test: inline in `types.rs` + confirm `report_schema_lockstep`

**Interfaces:**
- Produces: `Classification { system: String, id: String, vector: Option<String> }`; `FindingReport.classifications: Vec<Classification>` (`#[serde(default)]`); `FindingReport::all_classifications(&self) -> Vec<Classification>` folding legacy `cwe` (`CWE-###` ⇒ `{system:"CWE", id, vector:None}`) and `rating.cvss_v3` when it is a real score (`{system:"CVSS", id: cvss_v3.clone(), vector: None}`) together with explicit `classifications`, de-duplicated.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn cwe_and_explicit_classifications_fold_together() {
    let mut v = fixture(); // existing helper: valid_full.json
    v["cwe"] = serde_json::json!(["CWE-306"]);
    v["classifications"] = serde_json::json!([{"system":"CVE","id":"CVE-2026-0001","vector":"AV:N"}]);
    let r: FindingReport = serde_json::from_value(v).unwrap();
    let all = r.all_classifications();
    assert!(all.iter().any(|c| c.system == "CWE" && c.id == "CWE-306"));
    assert!(all.iter().any(|c| c.system == "CVE" && c.vector.as_deref() == Some("AV:N")));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rupu-coverage cwe_and_explicit_classifications_fold_together`
Expected: FAIL — unknown field `classifications` (`deny_unknown_fields`) / no `all_classifications`.

- [ ] **Step 3: Write minimal implementation**

In `src/report/types.rs`, add the type:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Classification {
    pub system: String,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vector: Option<String>,
}
```

Add the field to `FindingReport` (after `cwe`):

```rust
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub classifications: Vec<Classification>,
```

Add the accessor:

```rust
impl FindingReport {
    pub fn all_classifications(&self) -> Vec<Classification> {
        let mut out = self.classifications.clone();
        for id in &self.cwe {
            let c = Classification { system: "CWE".into(), id: id.clone(), vector: None };
            if !out.contains(&c) {
                out.push(c);
            }
        }
        let cvss = self.rating.cvss_v3.trim();
        if !cvss.is_empty() && !cvss.eq_ignore_ascii_case("unknown") {
            let c = Classification { system: "CVSS".into(), id: cvss.to_string(), vector: None };
            if !out.contains(&c) {
                out.push(c);
            }
        }
        out
    }
}
```

In `schema/finding_report.schema.json`, add to `properties` a `classifications` array (items: object with required `system`,`id`, optional `vector`), and — since the canonical schema forbids unknown fields via `additionalProperties:false` — this addition is what keeps lockstep. Do not mark it required.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p rupu-coverage cwe_and_explicit_classifications_fold_together report_schema_lockstep`
Expected: PASS both.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-coverage/src/report/types.rs crates/rupu-coverage/schema/finding_report.schema.json
git commit -m "feat(coverage): multi-system Classification on FindingReport (CWE/CVE/CVSS fold)"
```

---

## Task 5: EvidenceBlock on EvidenceClaim

**Files:**
- Modify: `crates/rupu-coverage/src/report/types.rs` (add `EvidenceBlock`, `blocks` field, `kind()`)
- Modify: `crates/rupu-coverage/schema/finding_report.schema.json` (describe claim `blocks`)
- Test: inline in `types.rs`

**Interfaces:**
- Consumes: `ArtifactRef` (existing).
- Produces: `EvidenceBlock` enum (`Text|CodeSlice|Diff|Table|Image|Hexdump|Disasm|Decompile|HttpExchange|ScanOutput|PcapRef`), `DisasmLine { addr: String, text: String }`, `EvidenceBlock::kind(&self) -> &'static str`; `EvidenceClaim.blocks: Vec<EvidenceBlock>` (`#[serde(default)]`).

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn claim_carries_typed_blocks_with_kind_tags() {
    let b = EvidenceBlock::Disasm {
        arch: "x86_64".into(),
        listing: vec![DisasmLine { addr: "0x401000".into(), text: "mov eax, edi".into() }],
    };
    assert_eq!(b.kind(), "disasm");
    let j = serde_json::to_value(&b).unwrap();
    assert_eq!(serde_json::from_value::<EvidenceBlock>(j).unwrap(), b);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rupu-coverage claim_carries_typed_blocks_with_kind_tags`
Expected: FAIL — `EvidenceBlock` not found.

- [ ] **Step 3: Write minimal implementation**

In `src/report/types.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisasmLine {
    pub addr: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "block", rename_all = "snake_case")]
pub enum EvidenceBlock {
    Text { text: String },
    CodeSlice { excerpt: String, #[serde(default, skip_serializing_if = "Option::is_none")] lang: Option<String> },
    Diff { diff: String },
    Table { headers: Vec<String>, rows: Vec<Vec<String>> },
    Image { artifact: String, #[serde(default, skip_serializing_if = "Option::is_none")] caption: Option<String> },
    Hexdump { base: u64, artifact: String, #[serde(default, skip_serializing_if = "Option::is_none")] rendered: Option<String> },
    Disasm { arch: String, listing: Vec<DisasmLine> },
    Decompile { lang: String, listing: String },
    HttpExchange { request: String, response: String },
    ScanOutput { tool: String, output: String },
    PcapRef { artifact: String, summary: String },
}

impl EvidenceBlock {
    pub fn kind(&self) -> &'static str {
        match self {
            EvidenceBlock::Text { .. } => "text",
            EvidenceBlock::CodeSlice { .. } => "code_slice",
            EvidenceBlock::Diff { .. } => "diff",
            EvidenceBlock::Table { .. } => "table",
            EvidenceBlock::Image { .. } => "image",
            EvidenceBlock::Hexdump { .. } => "hexdump",
            EvidenceBlock::Disasm { .. } => "disasm",
            EvidenceBlock::Decompile { .. } => "decompile",
            EvidenceBlock::HttpExchange { .. } => "http_exchange",
            EvidenceBlock::ScanOutput { .. } => "scan_output",
            EvidenceBlock::PcapRef { .. } => "pcap_ref",
        }
    }
}
```

Add to `EvidenceClaim` (after `artifact`):

```rust
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocks: Vec<EvidenceBlock>,
```

In the schema JSON, add `blocks` to the `evidence` items' `properties` as an array of objects with a `block` discriminator string (an unconstrained object array is acceptable — the Rust type is the enforcing contract; the schema addition only satisfies `additionalProperties:false` and lockstep). Not required.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p rupu-coverage claim_carries_typed_blocks_with_kind_tags report_schema_lockstep`
Expected: PASS both.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-coverage/src/report/types.rs crates/rupu-coverage/schema/finding_report.schema.json
git commit -m "feat(coverage): typed EvidenceBlock (incl. disasm/hexdump) on evidence claims"
```

---

## Task 6: Render disasm/hexdump blocks

**Files:**
- Modify: `crates/rupu-findings-report/src/blocks.rs` (map `EvidenceBlock` → existing `Block`)
- Test: inline in `blocks.rs`

**Interfaces:**
- Consumes: `rupu_coverage::report::EvidenceBlock`, `DisasmLine` (Task 5); existing `Block` model.
- Produces: `pub fn evidence_block_to_blocks(b: &EvidenceBlock) -> Vec<Block>` — disasm ⇒ `Block::Code{lang:Some("asm")}` (each line `addr  text`); hexdump ⇒ `Block::Code{lang:None}` (prefer `rendered`, else a note pointing at `artifact`); table ⇒ `Block::Table`; text ⇒ `Block::Prose`; code_slice ⇒ `Block::Code`; others ⇒ `Block::Prose`/`Block::Note` best-effort. Wired into the existing per-claim layout so `finding_blocks` emits them.

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod block_map_tests {
    use super::*;
    use crate::markdown;
    use rupu_coverage::report::{DisasmLine, EvidenceBlock};

    #[test]
    fn disasm_renders_as_asm_code_fence() {
        let b = EvidenceBlock::Disasm {
            arch: "x86_64".into(),
            listing: vec![DisasmLine { addr: "0x401000".into(), text: "mov eax, edi".into() }],
        };
        let md = markdown::render(&evidence_block_to_blocks(&b));
        assert!(md.contains("```asm"), "{md}");
        assert!(md.contains("0x401000  mov eax, edi"), "{md}");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rupu-findings-report block_map_tests`
Expected: FAIL — `evidence_block_to_blocks` not found.

- [ ] **Step 3: Write minimal implementation**

In `src/blocks.rs` (import `EvidenceBlock`, `DisasmLine` from `rupu_coverage::report`):

```rust
pub fn evidence_block_to_blocks(b: &EvidenceBlock) -> Vec<Block> {
    use rupu_coverage::report::EvidenceBlock as E;
    match b {
        E::Text { text } => vec![Block::Prose(text.clone())],
        E::CodeSlice { excerpt, lang } => vec![code(lang.as_deref(), excerpt)],
        E::Diff { diff } => vec![code(Some("diff"), diff)],
        E::Table { headers, rows } => vec![Block::Table { headers: headers.clone(), rows: rows.clone() }],
        E::Disasm { arch, listing } => {
            let body = listing.iter().map(|l| format!("{}  {}", l.addr, l.text)).collect::<Vec<_>>().join("\n");
            vec![Block::Note(format!("disassembly ({arch})")), code(Some("asm"), &body)]
        }
        E::Hexdump { base, artifact, rendered } => {
            let body = rendered.clone().unwrap_or_else(|| format!("(hexdump artifact: {artifact})"));
            vec![Block::Note(format!("hexdump @ {base:#x}")), code(None, &body)]
        }
        E::Decompile { lang, listing } => vec![code(Some(lang), listing)],
        E::HttpExchange { request, response } => {
            vec![code(Some("http"), request), code(Some("http"), response)]
        }
        E::ScanOutput { tool, output } => {
            vec![Block::Note(format!("scan: {tool}")), code(None, output)]
        }
        E::PcapRef { artifact, summary } => vec![Block::Note(format!("pcap {artifact}: {summary}"))],
        E::Image { artifact, caption } => {
            vec![Block::Note(format!("image {artifact}{}", caption.as_ref().map(|c| format!(" — {c}")).unwrap_or_default()))]
        }
    }
}
```

Then, in the existing per-claim layout inside `finding_blocks` (where each `EvidenceClaim`'s `excerpt` is emitted today), after the excerpt handling, append `for eb in &claim.blocks { out.extend(evidence_block_to_blocks(eb)); }`. Keep `code`/`Block` helpers already in the file.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p rupu-findings-report`
Expected: PASS (new test + existing renderer tests unaffected).

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-findings-report/src/blocks.rs
git commit -m "feat(findings-report): render disasm/hexdump evidence blocks via the Block model"
```

---

## Task 7: Predicate + completeness scoring

**Files:**
- Create: `crates/rupu-coverage/src/profile/predicate.rs`
- Create: `crates/rupu-coverage/src/profile/mod.rs` (module root; grows in Tasks 8–11)
- Modify: `crates/rupu-coverage/src/lib.rs` (add `pub mod profile;`)
- Test: inline in `predicate.rs`

**Interfaces:**
- Consumes: `FindingReport`, `EvidenceBlock` (Tasks 4–5); `Locator` (Task 1).
- Produces: `Predicate` enum (`HasField(String)|HasBlockKind(String)|HasClassificationSystem(String)|LocatorHasCoordinate(String)|MinSeverity(RiskLevel)|All(Vec<Predicate>)|Any(Vec<Predicate>)`); `CompletenessCheck { id, label, required, satisfied_when: Predicate }`; `evaluate(p: &Predicate, r: &FindingReport, loc: &Locator) -> Result<bool, PredicateError>`; `score(checks: &[CompletenessCheck], r, loc) -> Result<(u32, u32), PredicateError>` returning `(satisfied_required, total_required)`; `PredicateError` (`thiserror`, unknown field/coordinate/severity name).

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::asset::{Coordinate, Locator};
    // build_report(): a minimal valid FindingReport helper (see fixtures).

    #[test]
    fn predicates_evaluate_and_score() {
        let mut r = build_report();
        r.evidence[0].blocks = vec![rupu_coverage_block_disasm()];
        r.classifications = vec![cls("CWE", "CWE-306")];
        let loc = Locator(vec![Coordinate::Host("h".into()), Coordinate::Port { number: 1, proto: crate::asset::Proto::Tcp }]);

        assert!(evaluate(&Predicate::HasBlockKind("disasm".into()), &r, &loc).unwrap());
        assert!(evaluate(&Predicate::HasClassificationSystem("CWE".into()), &r, &loc).unwrap());
        assert!(evaluate(&Predicate::All(vec![
            Predicate::LocatorHasCoordinate("host".into()),
            Predicate::LocatorHasCoordinate("port".into()),
        ]), &r, &loc).unwrap());

        let checks = vec![
            CompletenessCheck { id: "listing".into(), label: "x".into(), required: true,
                satisfied_when: Predicate::HasBlockKind("disasm".into()) },
            CompletenessCheck { id: "http".into(), label: "y".into(), required: true,
                satisfied_when: Predicate::HasBlockKind("http_exchange".into()) },
        ];
        assert_eq!(score(&checks, &r, &loc).unwrap(), (1, 2));
    }

    #[test]
    fn unknown_field_is_an_error() {
        let r = build_report();
        let loc = Locator(vec![]);
        assert!(evaluate(&Predicate::HasField("nope".into()), &r, &loc).is_err());
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rupu-coverage profile::predicate`
Expected: FAIL — module/`Predicate` not found.

- [ ] **Step 3: Write minimal implementation**

`src/profile/mod.rs` (initial):

```rust
//! Engagement profiles: data-driven asset families layered on the primitives.
pub mod predicate;

pub use predicate::{score, CompletenessCheck, Predicate, PredicateError};
```

Add `pub mod profile;` in `lib.rs`.

`src/profile/predicate.rs`:

```rust
use crate::asset::Locator;
use crate::report::types::{FindingReport, RiskLevel};
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PredicateError {
    #[error("unknown report field in completeness predicate: {0}")]
    UnknownField(String),
    #[error("unknown coordinate tag in completeness predicate: {0}")]
    UnknownCoordinate(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Predicate {
    HasField(String),
    HasBlockKind(String),
    HasClassificationSystem(String),
    LocatorHasCoordinate(String),
    MinSeverity(RiskLevel),
    All(Vec<Predicate>),
    Any(Vec<Predicate>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletenessCheck {
    pub id: String,
    pub label: String,
    #[serde(default = "default_true")]
    pub required: bool,
    pub satisfied_when: Predicate,
}

fn default_true() -> bool {
    true
}

/// The report fields a `has_field` predicate may name (present & non-empty /
/// non-sentinel). Unknown names are an error, never a silent `false`.
fn field_present(r: &FindingReport, name: &str) -> Result<bool, PredicateError> {
    let ok = match name {
        "root_cause" => !r.root_cause.trim().is_empty(),
        "remediation" => !r.remediation.trim().is_empty(),
        "impact" => !r.impact.trim().is_empty(),
        "description" => !r.description.trim().is_empty(),
        "attack_vector" => !r.attack_vector.trim().is_empty(),
        "category" => !r.category.trim().is_empty(),
        "replication_steps" => !r.replication_steps.is_empty(),
        "evidence" => !r.evidence.is_empty(),
        _ => return Err(PredicateError::UnknownField(name.to_string())),
    };
    Ok(ok)
}

pub fn evaluate(p: &Predicate, r: &FindingReport, loc: &Locator) -> Result<bool, PredicateError> {
    Ok(match p {
        Predicate::HasField(f) => field_present(r, f)?,
        Predicate::HasBlockKind(k) => r.evidence.iter().any(|c| c.blocks.iter().any(|b| b.kind() == k)),
        Predicate::HasClassificationSystem(s) => {
            r.all_classifications().iter().any(|c| c.system.eq_ignore_ascii_case(s))
        }
        Predicate::LocatorHasCoordinate(tag) => {
            if !crate::asset::Coordinate::known_tag(tag) {
                return Err(PredicateError::UnknownCoordinate(tag.clone()));
            }
            loc.has(tag)
        }
        Predicate::MinSeverity(min) => rank(r.rating.risk_rating) >= rank(*min),
        Predicate::All(ps) => {
            for q in ps { if !evaluate(q, r, loc)? { return Ok(false); } }
            true
        }
        Predicate::Any(ps) => {
            for q in ps { if evaluate(q, r, loc)? { return Ok(true); } }
            false
        }
    })
}

fn rank(l: RiskLevel) -> u8 {
    match l { RiskLevel::Low => 0, RiskLevel::Medium => 1, RiskLevel::High => 2, RiskLevel::Critical => 3 }
}

/// `(satisfied_required, total_required)`.
pub fn score(checks: &[CompletenessCheck], r: &FindingReport, loc: &Locator) -> Result<(u32, u32), PredicateError> {
    let mut total = 0;
    let mut ok = 0;
    for c in checks.iter().filter(|c| c.required) {
        total += 1;
        if evaluate(&c.satisfied_when, r, loc)? { ok += 1; }
    }
    Ok((ok, total))
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p rupu-coverage profile::predicate`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-coverage/src/profile crates/rupu-coverage/src/lib.rs
git commit -m "feat(coverage): completeness Predicate vocabulary + scoring"
```

---

## Task 8: EngagementProfile package + TOML parse

**Files:**
- Modify: `crates/rupu-coverage/src/profile/mod.rs` (add `EngagementProfile`, `AssetKindDef`, `CoverageSpec`, `Bundle`)
- Create: `crates/rupu-coverage/src/profile/package.rs` (parse + validate)
- Test: inline in `package.rs`

**Interfaces:**
- Consumes: `CompletenessCheck` (Task 7); `Coordinate::known_tag` (Task 1).
- Produces: `EngagementProfile { id, name, includes: Vec<String>, asset_kinds: Vec<AssetKindDef>, evidence_blocks: Vec<String>, classification_systems: Vec<String>, completeness: Vec<CompletenessCheck>, coverage: CoverageSpec, bundle: Bundle }`; `AssetKindDef { id, parent: Option<String>, coordinates: Vec<String>, attributes: Vec<String>, label: String }`; `CoverageSpec { enumerates: Vec<String>, depth_ladder: Vec<String> }`; `Bundle { agents, tools, workflows: Vec<String> }`; `parse_profile(toml_src: &str) -> Result<EngagementProfile, ProfileError>` (rejects unknown coordinate tags in any kind, fail-closed); `ProfileError` (`thiserror`).

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const BINARY: &str = include_str!("builtin/binary.toml");

    #[test]
    fn parses_the_builtin_binary_profile() {
        let p = parse_profile(BINARY).unwrap();
        assert_eq!(p.id, "binary");
        assert!(p.asset_kinds.iter().any(|k| k.id == "function"
            && k.coordinates.contains(&"address".to_string())));
        assert_eq!(p.coverage.depth_ladder.first().map(String::as_str), Some("located"));
    }

    #[test]
    fn unknown_coordinate_is_rejected() {
        let bad = r#"
id = "x"
name = "x"
[[asset_kinds]]
id = "k"
coordinates = ["nonsense"]
label = "l"
[coverage]
enumerates = ["k"]
depth_ladder = ["a"]
[bundle]
"#;
        assert!(matches!(parse_profile(bad), Err(ProfileError::UnknownCoordinate(_))));
    }
}
```

(Task 8 depends on `builtin/binary.toml`, authored in Task 11. To keep Task 8 self-contained, create a minimal `builtin/binary.toml` now with just `binary` + `function` kinds and the coverage/bundle blocks; Task 11 fills in completeness/blocks/classification. If executing strictly in order, write that minimal file as Step 3 here.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rupu-coverage profile::package`
Expected: FAIL — `parse_profile` not found.

- [ ] **Step 3: Write minimal implementation**

Add to `src/profile/mod.rs`:

```rust
pub mod package;
pub use package::{parse_profile, AssetKindDef, Bundle, CoverageSpec, EngagementProfile, ProfileError};
```

`src/profile/package.rs`:

```rust
use crate::asset::Coordinate;
use crate::profile::predicate::CompletenessCheck;
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum ProfileError {
    #[error("profile TOML parse error: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("kind {kind:?} names unknown coordinate {tag:?}")]
    UnknownCoordinate { kind: String, tag: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetKindDef {
    pub id: String,
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default)]
    pub coordinates: Vec<String>,
    #[serde(default)]
    pub attributes: Vec<String>,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoverageSpec {
    #[serde(default)]
    pub enumerates: Vec<String>,
    #[serde(default)]
    pub depth_ladder: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bundle {
    #[serde(default)]
    pub agents: Vec<String>,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub workflows: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngagementProfile {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub includes: Vec<String>,
    #[serde(default)]
    pub asset_kinds: Vec<AssetKindDef>,
    #[serde(default)]
    pub evidence_blocks: Vec<String>,
    #[serde(default)]
    pub classification_systems: Vec<String>,
    #[serde(default)]
    pub completeness: Vec<CompletenessCheck>,
    #[serde(default)]
    pub coverage: CoverageSpec,
    #[serde(default)]
    pub bundle: Bundle,
}

impl Default for CoverageSpec {
    fn default() -> Self {
        CoverageSpec { enumerates: vec![], depth_ladder: vec![] }
    }
}

pub fn parse_profile(src: &str) -> Result<EngagementProfile, ProfileError> {
    let p: EngagementProfile = toml::from_str(src)?;
    for k in &p.asset_kinds {
        for tag in &k.coordinates {
            if !Coordinate::known_tag(tag) {
                return Err(ProfileError::UnknownCoordinate { kind: k.id.clone(), tag: tag.clone() });
            }
        }
    }
    Ok(p)
}
```

Add the `UnknownCoordinate` variant name used by the test (`ProfileError::UnknownCoordinate { .. }`); the test's `matches!` uses the struct variant.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p rupu-coverage profile::package`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-coverage/src/profile
git commit -m "feat(coverage): EngagementProfile data package + TOML parse (fail-closed on unknown coordinate)"
```

---

## Task 9: Loader — embedded + discovery + composite expansion

**Files:**
- Create: `crates/rupu-coverage/src/profile/loader.rs`
- Modify: `crates/rupu-coverage/src/profile/mod.rs`
- Test: inline in `loader.rs`

**Interfaces:**
- Consumes: `parse_profile`, `EngagementProfile` (Task 8).
- Produces: `expand_includes(all: &BTreeMap<String, EngagementProfile>, id: &str) -> Result<EngagementProfile, LoadError>` — recursively unions an `includes` chain (asset_kinds/evidence_blocks/classification_systems/completeness/coverage merged; the composite's own name/id win; cycles and missing includes are errors); `discover(dirs: &[PathBuf]) -> BTreeMap<String, EngagementProfile>` — later dirs override earlier by id (precedence: embedded < global < project, so caller orders dirs accordingly); `LoadError` (`thiserror`, `IncludeCycle`, `MissingInclude`, from `ProfileError`).

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn prof(id: &str, includes: &[&str], kind: &str) -> EngagementProfile {
        let mut p = crate::profile::parse_profile(&format!(
            "id=\"{id}\"\nname=\"{id}\"\n[[asset_kinds]]\nid=\"{kind}\"\nlabel=\"l\"\n[coverage]\n[bundle]\n"
        )).unwrap();
        p.includes = includes.iter().map(|s| s.to_string()).collect();
        p
    }

    #[test]
    fn composite_unions_included_kinds() {
        let mut all = BTreeMap::new();
        all.insert("network".into(), prof("network", &[], "service"));
        all.insert("web".into(), prof("web", &[], "route"));
        all.insert("pentest".into(), prof("pentest", &["network", "web"], "extra"));

        let p = expand_includes(&all, "pentest").unwrap();
        let ids: Vec<_> = p.asset_kinds.iter().map(|k| k.id.as_str()).collect();
        assert!(ids.contains(&"service") && ids.contains(&"route") && ids.contains(&"extra"));
        assert_eq!(p.id, "pentest");
    }

    #[test]
    fn cycles_and_missing_are_errors() {
        let mut all = BTreeMap::new();
        all.insert("a".into(), prof("a", &["b"], "ka"));
        all.insert("b".into(), prof("b", &["a"], "kb"));
        assert!(matches!(expand_includes(&all, "a"), Err(LoadError::IncludeCycle(_))));

        let mut m = BTreeMap::new();
        m.insert("x".into(), prof("x", &["missing"], "kx"));
        assert!(matches!(expand_includes(&m, "x"), Err(LoadError::MissingInclude(_))));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rupu-coverage profile::loader`
Expected: FAIL — `expand_includes` not found.

- [ ] **Step 3: Write minimal implementation**

Add to `src/profile/mod.rs`: `pub mod loader;` and `pub use loader::{discover, expand_includes, LoadError};`.

`src/profile/loader.rs`:

```rust
use crate::profile::{parse_profile, EngagementProfile, ProfileError};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error("profile include cycle at {0:?}")]
    IncludeCycle(String),
    #[error("profile {0:?} includes unknown profile")]
    MissingInclude(String),
    #[error(transparent)]
    Profile(#[from] ProfileError),
}

/// Recursively merge `id`'s `includes` into one flattened profile.
pub fn expand_includes(
    all: &BTreeMap<String, EngagementProfile>,
    id: &str,
) -> Result<EngagementProfile, LoadError> {
    fn go(
        all: &BTreeMap<String, EngagementProfile>,
        id: &str,
        stack: &mut Vec<String>,
    ) -> Result<EngagementProfile, LoadError> {
        if stack.iter().any(|s| s == id) {
            return Err(LoadError::IncludeCycle(id.to_string()));
        }
        let base = all.get(id).ok_or_else(|| LoadError::MissingInclude(id.to_string()))?;
        let mut merged = base.clone();
        stack.push(id.to_string());
        for inc in &base.includes {
            let sub = go(all, inc, stack).map_err(|e| match e {
                LoadError::MissingInclude(_) => LoadError::MissingInclude(id.to_string()),
                other => other,
            })?;
            merged.asset_kinds.extend(sub.asset_kinds);
            merged.evidence_blocks.extend(sub.evidence_blocks);
            merged.classification_systems.extend(sub.classification_systems);
            merged.completeness.extend(sub.completeness);
            merged.coverage.enumerates.extend(sub.coverage.enumerates);
            merged.coverage.depth_ladder.extend(sub.coverage.depth_ladder);
        }
        stack.pop();
        merged.includes.clear();
        dedup(&mut merged);
        Ok(merged)
    }
    go(all, id, &mut Vec::new())
}

fn dedup(p: &mut EngagementProfile) {
    p.evidence_blocks.sort();
    p.evidence_blocks.dedup();
    p.classification_systems.sort();
    p.classification_systems.dedup();
    // asset_kinds keep insertion order; drop later duplicates by id.
    let mut seen = std::collections::HashSet::new();
    p.asset_kinds.retain(|k| seen.insert(k.id.clone()));
}

/// Discover `*.toml` profiles across dirs; later dirs override earlier by id.
pub fn discover(dirs: &[PathBuf]) -> BTreeMap<String, EngagementProfile> {
    let mut out = BTreeMap::new();
    for dir in dirs {
        let Ok(rd) = std::fs::read_dir(dir) else { continue };
        for entry in rd.flatten() {
            let path: &Path = &entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("toml") {
                continue;
            }
            let Ok(src) = std::fs::read_to_string(path) else { continue };
            if let Ok(p) = parse_profile(&src) {
                out.insert(p.id.clone(), p);
            }
        }
    }
    out
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p rupu-coverage profile::loader`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-coverage/src/profile
git commit -m "feat(coverage): profile loader — discovery precedence + composite include expansion"
```

---

## Task 10: ProfileRegistry — active set, route-by-kind, narrow-only

**Files:**
- Create: `crates/rupu-coverage/src/profile/registry.rs`
- Modify: `crates/rupu-coverage/src/profile/mod.rs`
- Test: inline in `registry.rs`

**Interfaces:**
- Consumes: `EngagementProfile` (Task 8), `expand_includes` (Task 9), `profile_of` (Task 2).
- Produces: `ProfileRegistry` holding flattened profiles keyed by id, each with **namespaced** kind ids; `ProfileRegistry::from_profiles(BTreeMap<String, EngagementProfile>) -> Result<Self, RegistryError>` (expands composites, namespaces kinds `<id>:<kind>`, errors on a namespaced-kind collision across profiles); `active_set(&self, ids: &[String]) -> Result<ActiveSet, RegistryError>`; `ActiveSet::profile_for_kind(&self, namespaced_kind: &str) -> Option<&EngagementProfile>`; `ActiveSet::narrow(&self, subset: &[String]) -> Result<ActiveSet, RegistryError>` (rejects any id not already in the set — narrow only); `RegistryError` (`thiserror`, `UnknownProfile`, `KindCollision`, `WidenNotAllowed`, from `LoadError`).

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::parse_profile;
    use std::collections::BTreeMap;

    fn p(id: &str, kind: &str) -> crate::profile::EngagementProfile {
        parse_profile(&format!("id=\"{id}\"\nname=\"{id}\"\n[[asset_kinds]]\nid=\"{kind}\"\nlabel=\"l\"\n[coverage]\n[bundle]\n")).unwrap()
    }

    #[test]
    fn routes_by_namespaced_kind_and_narrows_only() {
        let mut all = BTreeMap::new();
        all.insert("network".into(), p("network", "service"));
        all.insert("web".into(), p("web", "route"));
        let reg = ProfileRegistry::from_profiles(all).unwrap();

        let set = reg.active_set(&["network".into(), "web".into()]).unwrap();
        assert_eq!(set.profile_for_kind("network:service").unwrap().id, "network");
        assert_eq!(set.profile_for_kind("web:route").unwrap().id, "web");
        assert!(set.profile_for_kind("cloud:bucket").is_none());

        let narrowed = set.narrow(&["network".into()]).unwrap();
        assert!(narrowed.profile_for_kind("web:route").is_none());
        assert!(matches!(set.narrow(&["cloud".into()]), Err(RegistryError::WidenNotAllowed(_))));
    }

    #[test]
    fn unknown_profile_errors() {
        let reg = ProfileRegistry::from_profiles(BTreeMap::new()).unwrap();
        assert!(matches!(reg.active_set(&["ghost".into()]), Err(RegistryError::UnknownProfile(_))));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rupu-coverage profile::registry`
Expected: FAIL — `ProfileRegistry` not found.

- [ ] **Step 3: Write minimal implementation**

Add to `src/profile/mod.rs`: `pub mod registry;` and `pub use registry::{ActiveSet, ProfileRegistry, RegistryError};`.

`src/profile/registry.rs`:

```rust
use crate::asset::types::profile_of;
use crate::profile::loader::{expand_includes, LoadError};
use crate::profile::EngagementProfile;
use std::collections::BTreeMap;

#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("unknown engagement profile: {0}")]
    UnknownProfile(String),
    #[error("kind {0:?} is declared by more than one active profile")]
    KindCollision(String),
    #[error("a step may only narrow the active set; {0:?} is not in it")]
    WidenNotAllowed(String),
    #[error(transparent)]
    Load(#[from] LoadError),
}

pub struct ProfileRegistry {
    /// Flattened (composites expanded), kinds namespaced `<id>:<kind>`.
    profiles: BTreeMap<String, EngagementProfile>,
}

impl ProfileRegistry {
    pub fn from_profiles(raw: BTreeMap<String, EngagementProfile>) -> Result<Self, RegistryError> {
        let mut profiles = BTreeMap::new();
        for id in raw.keys() {
            let mut flat = expand_includes(&raw, id)?;
            for k in &mut flat.asset_kinds {
                k.id = format!("{}:{}", flat.id, k.id);
                if let Some(parent) = &mut k.parent {
                    *parent = format!("{}:{}", flat.id, parent);
                }
            }
            profiles.insert(id.clone(), flat);
        }
        Ok(ProfileRegistry { profiles })
    }

    pub fn active_set(&self, ids: &[String]) -> Result<ActiveSet, RegistryError> {
        let mut chosen = Vec::new();
        for id in ids {
            let p = self.profiles.get(id).ok_or_else(|| RegistryError::UnknownProfile(id.clone()))?;
            chosen.push(p.clone());
        }
        // Fail-closed on a namespaced-kind collision across the active set.
        let mut seen = std::collections::HashSet::new();
        for p in &chosen {
            for k in &p.asset_kinds {
                if !seen.insert(k.id.clone()) {
                    return Err(RegistryError::KindCollision(k.id.clone()));
                }
            }
        }
        Ok(ActiveSet { profiles: chosen })
    }
}

pub struct ActiveSet {
    profiles: Vec<EngagementProfile>,
}

impl ActiveSet {
    pub fn ids(&self) -> Vec<&str> {
        self.profiles.iter().map(|p| p.id.as_str()).collect()
    }

    pub fn profile_for_kind(&self, namespaced_kind: &str) -> Option<&EngagementProfile> {
        let owner = profile_of(namespaced_kind);
        self.profiles.iter().find(|p| p.id == owner)
    }

    pub fn narrow(&self, subset: &[String]) -> Result<ActiveSet, RegistryError> {
        let mut kept = Vec::new();
        for id in subset {
            match self.profiles.iter().find(|p| &p.id == id) {
                Some(p) => kept.push(p.clone()),
                None => return Err(RegistryError::WidenNotAllowed(id.clone())),
            }
        }
        Ok(ActiveSet { profiles: kept })
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p rupu-coverage profile::registry`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-coverage/src/profile
git commit -m "feat(coverage): ProfileRegistry — active set, route-by-namespaced-kind, narrow-only"
```

---

## Task 11: Built-in binary profile + native code descriptor + defaults

**Files:**
- Create/replace: `crates/rupu-coverage/src/profile/builtin/binary.toml` (full)
- Modify: `crates/rupu-coverage/src/profile/mod.rs` (add `code_profile()`, `builtin_registry()`)
- Test: inline in `mod.rs`

**Interfaces:**
- Consumes: everything above.
- Produces: `code_profile() -> EngagementProfile` (native, unqualified kind `file`, coordinates path/line_range/symbol, ladder `unreviewed→reviewed`, empty completeness so existing behavior is unchanged); `builtin_registry() -> ProfileRegistry` (registers `code` native + embedded `binary.toml`); default active profile id `"code"`.

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod builtin_tests {
    use super::*;

    #[test]
    fn builtin_registry_has_code_and_binary_namespaced() {
        let reg = builtin_registry().unwrap();
        let set = reg.active_set(&["code".into(), "binary".into()]).unwrap();
        assert_eq!(set.profile_for_kind("binary:function").unwrap().id, "binary");
        assert_eq!(set.profile_for_kind("code:file").unwrap().id, "code");
    }

    #[test]
    fn binary_profile_completeness_requires_a_listing() {
        let reg = builtin_registry().unwrap();
        let set = reg.active_set(&["binary".into()]).unwrap();
        let bin = set.profile_for_kind("binary:function").unwrap();
        assert!(bin.completeness.iter().any(|c| c.id == "evidence_has_listing"));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rupu-coverage builtin_tests`
Expected: FAIL — `builtin_registry` / `code_profile` not found.

- [ ] **Step 3: Write minimal implementation**

`src/profile/builtin/binary.toml` (full — synthetic, no assessment data):

```toml
id = "binary"
name = "Binary reverse engineering"

[[asset_kinds]]
id = "binary"
coordinates = ["sha256"]
label = "{sha256}"

[[asset_kinds]]
id = "function"
parent = "binary"
coordinates = ["sha256", "address", "symbol"]
label = "{symbol} @ {address}"

evidence_blocks = ["text", "code_slice", "diff", "hexdump", "disasm"]
classification_systems = ["CWE", "CVE"]

[[completeness]]
id = "evidence_has_listing"
label = "Evidence includes a disassembly or hexdump"
required = true
satisfied_when = { any = [ { has_block_kind = "disasm" }, { has_block_kind = "hexdump" } ] }

[[completeness]]
id = "has_root_cause"
label = "Root cause stated"
required = true
satisfied_when = { has_field = "root_cause" }

[[completeness]]
id = "classified"
label = "Weakness classified (CWE)"
required = true
satisfied_when = { has_classification_system = "CWE" }

[coverage]
enumerates = ["binary", "function"]
depth_ladder = ["located", "disassembled", "analyzed"]

[bundle]
agents = ["binary-triage", "binary-analyst"]
tools = ["mcp:radare2", "mcp:ghidra"]
workflows = ["binary-assessment"]
```

Add to `src/profile/mod.rs`:

```rust
pub const DEFAULT_PROFILE: &str = "code";

pub fn code_profile() -> EngagementProfile {
    EngagementProfile {
        id: "code".into(),
        name: "Secure code review".into(),
        includes: vec![],
        asset_kinds: vec![AssetKindDef {
            id: "file".into(),
            parent: None,
            coordinates: vec!["path".into(), "line_range".into(), "symbol".into()],
            attributes: vec![],
            label: "{path}".into(),
        }],
        evidence_blocks: vec!["text".into(), "code_slice".into(), "diff".into()],
        classification_systems: vec!["CWE".into()],
        completeness: vec![], // native path is unchanged; no profile-driven checks yet
        coverage: CoverageSpec {
            enumerates: vec!["file".into()],
            depth_ladder: vec!["unreviewed".into(), "reviewed".into()],
        },
        bundle: Bundle::default(),
    }
}

pub fn builtin_registry() -> Result<registry::ProfileRegistry, registry::RegistryError> {
    let mut raw = std::collections::BTreeMap::new();
    raw.insert("code".to_string(), code_profile());
    let binary = package::parse_profile(include_str!("builtin/binary.toml"))
        .expect("embedded binary profile parses");
    raw.insert("binary".to_string(), binary);
    registry::ProfileRegistry::from_profiles(raw)
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p rupu-coverage builtin_tests profile::`
Expected: PASS.

- [ ] **Step 5: Full-crate gate + commit**

```bash
cargo test -p rupu-coverage && cargo test -p rupu-findings-report
cargo clippy -p rupu-coverage -p rupu-findings-report --all-targets
git add crates/rupu-coverage/src/profile
git commit -m "feat(coverage): built-in binary profile + native code descriptor + default registry"
```

---

## Self-Review

**1. Spec coverage.**
- Primitive vocabulary (`Coordinate`, `EvidenceBlock`, `Predicate`) → Tasks 1, 5, 7. ✔
- Asset + graph, profile-namespaced kinds, `Locator(vec![])` valid → Tasks 2, 3. ✔
- Classification generalization + cwe/cvss fold → Task 4. ✔
- disasm/hexdump renderers → Task 6. ✔
- Completeness as data + tiny predicate set incl. `LocatorHasCoordinate` → Task 7. ✔
- Profile package + loader + discovery precedence + composite `includes` → Tasks 8, 9. ✔
- Active-set + route-by-kind + narrow-only + fail-closed → Task 10. ✔
- Built-in `binary` (data) + `code` (native, registered through the seam) → Task 11. ✔
- **Deferred to Plan 2 (wiring), by design:** JSONL persistence of `AssetGraph`; CLI/workflow/agent-frontmatter/session/MCP selection; profile-aware `report_finding`/`coverage_mark`; scope/RoE enforcement; end-to-end binary agent + workflow. **Deferred to Plan 3:** the `network` profile and live-scope enforcement.

**2. Placeholder scan.** No "TBD"/"add error handling"/"similar to Task N": every step carries real code. Cross-task type names checked below.

**3. Type consistency.** `Coordinate::known_tag`/`tag` (T1) used by T7/T8; `profile_of` (T2) used by T10; `EngagementProfile`/`AssetKindDef`/`CoverageSpec`/`Bundle` (T8) used by T9/T10/T11; `CompletenessCheck`/`Predicate` (T7) used by T8/T11; `EvidenceBlock::kind` (T5) used by T6/T7; `expand_includes`/`LoadError` (T9) used by T10; `parse_profile` (T8) used by T9/T11. Names consistent throughout.

---

## Follow-on plans (not in this plan)

- **Plan 2 — wiring + end-to-end:** persist `AssetGraph` (JSONL beside `run.json`); `rupu run --engagement-profile(s)` + workflow `defaults.engagement_profiles` + agent frontmatter `engagementProfiles` + session/MCP selection with `FindingProfile`-style precedence; profile-aware `report_finding`/`coverage_mark` (validate by the finding's asset-kind namespace, compute completeness, write assets + depth); a sample `binary` agent + `binary-assessment` workflow under `.rupu/`; a mock-provider end-to-end test asserting a finding with a rendered disasm block.
- **Plan 3 — network fast-follow:** `network` profile as data, `host`/`port`/`url` coordinates already exist; `http_exchange`/`scan_output`/`pcap_ref` renderers; scope-as-root RoE enforcement (live-touch tools checked against the in-scope asset set, fail-closed).

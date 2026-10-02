# Engagement Profiles — Implementation Plan (single plan · single pass · from scratch · fully specified)

> **For agentic workers:** execute top to bottom in one pass. This document contains every type definition and every profile verbatim — implementation is transcription + wiring + tests, not design.

**Goal:** Make rupu's finding/coverage layer agnostic to the *type of asset* (code / binary / host / service / function / package / cloud resource / …) via **data-driven engagement profiles**, and ship the **entire** built-in profile catalog from the spec — so running any engagement is picking a profile, not writing Rust.

**Approach:** Delete every line of prior engagement work (Plan 1 #687 — merged, so reverted; Plan 2 #709 — closed; Plan 3 — local, gone) and build the whole system once. The core owns a fixed set of typed primitives (coordinates, evidence blocks, classification, a tiny completeness predicate vocabulary); a **profile is a pure-data TOML package** composing them. A finding is **about an asset with a profile-namespaced kind** and routes to its owning profile for validation. A run may activate several profiles at once (`pentest` = `network` + `web`); findings route by asset-kind namespace. **Agents do the work with bash and whatever tools they want** — rupu records and validates *findings*; it does not run, gate, or sandbox tools.

**Spec:** `docs/superpowers/specs/2026-09-30-rupu-engagement-profiles-asset-model-design.md` (cleaned of scope/RoE in Step 1).

## OUT of scope — never build
No scope-as-root RoE, no `enforce_in_scope`, no `[scope]` blocks, no `cidrs`/`out_of_scope`/`window`, no live-touch/target checking, no sandboxing/egress control, no `http_request` tool, no bash restriction/stripping, no `ScopePolicy`/CIDR engine, no engagement "refusal" guards. The only validation: an unknown profile id is a hard error; a finding whose asset kind no active profile owns is rejected at write time. That is finding validation — not traffic control.

## Global Constraints
- Hexagonal; `rupu-cli` thin. `#![deny(clippy::all)]`, `#![forbid(unsafe_code)]`. Workspace deps only. `thiserror` libs / `anyhow` CLI.
- **Targeted tests only** (`cargo test -p <crate>`); `--workspace` hangs here. Re-bless the MCP snapshot when the catalog changes.
- Profile TOML: all profile-level scalar/array keys precede the first `[[asset_kinds]]` (serde `deny_unknown_fields`). Built-ins via `include_str!`; overlay built-in `<` `~/.rupu/profiles/` `<` `.rupu/profiles/`.
- Back-compat: `cwe`/`cvss_v3`/`excerpt`/`lang`/`binary_va` keep deserializing; a legacy finding with no asset kind → `code`.
- Report-model lockstep sites: model · embedded draft-07 schema · `validate_report` · web DTO · MCP tool schema snapshot.
- Commits local; one push at the very end to a fresh PR.

---

# Part I — Type definitions (the core ABI)

### `crates/rupu-coverage/src/asset/coordinate.rs`
```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Proto { Tcp, Udp, Other }

/// Typed locator primitives. Adding a variant is the ONLY per-kind core change.
/// All anticipated variants are declared up front; an unused one costs nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "t", content = "v")]
pub enum Coordinate {
    Path(String),
    LineRange { start: u32, end: u32 },
    Symbol(String),
    Commit(String),
    Sha256(String),
    Offset(u64),
    Address(u64),
    Host(String),
    Port { number: u16, proto: Proto },
    Url(String),
    HttpRoute { method: String, path: String },
    Param(String),
    ResourceId { scheme: String, id: String },
}

impl Coordinate {
    pub fn tag(&self) -> &'static str { /* "path"|"line_range"|…|"resource_id" */ }
    pub fn known_tag(tag: &str) -> bool { /* matches any variant tag */ }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Locator(pub Vec<Coordinate>);
impl Locator { pub fn has(&self, tag: &str) -> bool { self.0.iter().any(|c| c.tag() == tag) } }
```

### `crates/rupu-coverage/src/asset/mod.rs`
```rust
pub type AssetId = String; // "<kind>:<sha256(kind|locator-canonical)>", stable across re-runs

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Asset {
    pub id: AssetId,
    pub kind: String,                 // profile-namespaced, e.g. "network:service"
    pub parent: Option<AssetId>,      // roots == top-level assets (NOT scope)
    pub locator: Locator,
    pub label: String,                // rendered from the kind's label template
    #[serde(default)]
    pub depth: Option<String>,        // profile-declared coverage depth state
    #[serde(default)]
    pub attributes: BTreeMap<String, serde_json::Value>,
}

/// "network:service" -> "network"
pub fn profile_of(namespaced_kind: &str) -> &str { namespaced_kind.split(':').next().unwrap_or(namespaced_kind) }
```
`asset/store.rs`: append-only `assets.jsonl` under the workspace coverage dir; atomic append; fold-on-read last-write-wins by `id`; `upsert_asset`, `from_assets`, iterator.

### `crates/rupu-coverage/src/report/types.rs` (evidence + classification)
```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum EvidenceBlock {
    Text { text: String },
    CodeSlice { locator: Locator, excerpt: String, lang: Option<String> },
    Diff { diff: String },
    Table { headers: Vec<String>, rows: Vec<Vec<String>> },
    Image { artifact: ArtifactRef, caption: Option<String> },
    Hexdump { base: u64, artifact: ArtifactRef, rendered: Option<String> },
    Disasm { arch: String, listing: Vec<DisasmLine> },
    Decompile { lang: String, listing: String },
    HttpExchange { request: String, response: String },
    ScanOutput { tool: String, output: String },
    PcapRef { artifact: ArtifactRef, summary: String },
}
impl EvidenceBlock { pub fn kind(&self) -> &'static str { /* serialized tag */ } }

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisasmLine { pub address: u64, pub bytes: String, pub mnemonic: String, pub ops: String }

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Classification { pub system: String, pub id: String, pub vector: Option<String> }
```
Back-compat: legacy `excerpt`/`lang` on a claim → `CodeSlice`; `binary_va` → `Address`; `cwe: Vec<String>` + `cvss_v3` fold into `Vec<Classification>` and keep deserializing. Renderers (`blocks/markdown/html/typst_doc/prose`) get an arm per variant: `scan_output`/`pcap_ref` reuse text + blob store; `table`/`image` generic; `hexdump`/`disasm`/`decompile` real.

### `crates/rupu-coverage/src/profile/predicate.rs`
```rust
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
pub struct CompletenessCheck { pub id: String, pub label: String,
    #[serde(default = "crate::profile::predicate::default_true")] pub required: bool,
    pub satisfied_when: Predicate }

pub fn evaluate(p: &Predicate, r: &FindingReport, loc: &Locator) -> Result<bool, PredicateError>;
// HasField names: root_cause|remediation|impact|description|attack_vector|category|replication_steps|evidence
// unknown field / coordinate / block name => PredicateError (never silent false)
```

### `crates/rupu-coverage/src/profile/package.rs`
```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EngagementProfile {
    pub id: String,
    pub name: String,
    #[serde(default)] pub includes: Vec<String>,             // composite expansion
    #[serde(default)] pub evidence_blocks: Vec<String>,      // declaration (advisory)
    #[serde(default)] pub classification_systems: Vec<String>,
    #[serde(default)] pub asset_kinds: Vec<AssetKindDef>,
    #[serde(default)] pub completeness: Vec<CompletenessCheck>,
    #[serde(default)] pub coverage: Coverage,
    #[serde(default)] pub bundle: Bundle,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssetKindDef {
    pub id: String,
    #[serde(default)] pub parent: Option<String>,
    #[serde(default)] pub coordinates: Vec<String>,          // coordinate tags this kind carries
    pub label: String,                                       // template, e.g. "{host}:{port}"
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Coverage { #[serde(default)] pub enumerates: Vec<String>, #[serde(default)] pub depth_ladder: Vec<String> }

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bundle { #[serde(default)] pub agents: Vec<String>, #[serde(default)] pub tools: Vec<String>, #[serde(default)] pub workflows: Vec<String> }

pub fn parse_profile(toml_src: &str) -> Result<EngagementProfile, ProfileError>;
```

### `crates/rupu-coverage/src/profile/{loader,registry,mod}.rs`
```rust
// loader.rs
pub fn expand_includes(all: &BTreeMap<String,EngagementProfile>, id: &str) -> Result<EngagementProfile, LoadError>; // union kinds/blocks/tax/completeness/coverage
pub fn include_closure(all: &BTreeMap<String,EngagementProfile>, id: &str) -> Result<Vec<String>, LoadError>;
pub fn discover(dirs: &[PathBuf]) -> (BTreeMap<String,EngagementProfile>, Vec<String>); // fail-closed on unparseable

// registry.rs
pub struct ProfileRegistry { entries: BTreeMap<String, Entry> }   // Entry { profile, closure }
impl ProfileRegistry {
    pub fn from_profiles(raw: BTreeMap<String,EngagementProfile>) -> Result<Self, RegistryError>; // namespaces kinds "<id>:<kind>"
    pub fn active_set(&self, ids: &[String]) -> Result<ActiveSet, RegistryError>;                 // composite expands; kind-collision across selections = error
}
pub struct ActiveSet { selected: Vec<String>, entries: BTreeMap<String, Entry> }
impl ActiveSet {
    pub fn ids(&self) -> Vec<&str>;
    pub fn profiles(&self) -> impl Iterator<Item=&EngagementProfile>;
    pub fn profile_for_kind(&self, namespaced_kind: &str) -> Option<&EngagementProfile>; // members keep own namespace under a composite
    pub fn narrow(&self, subset: &[String]) -> Result<ActiveSet, RegistryError>;          // narrow-only; widening = error
}

// mod.rs
pub fn builtin_profiles() -> BTreeMap<String, EngagementProfile>;   // code (native) + include_str! of every builtin/*.toml
pub fn builtin_registry() -> Result<ProfileRegistry, RegistryError>;
pub fn registry_with_overlay(global: &Path, project: &Path) -> Result<ProfileRegistry, RegistryError>;
pub const DEFAULT_PROFILE: &str = "code";
```

### `report/options.rs` + `tools/report_finding.rs` + `tools/asset_mark.rs`
```rust
pub struct FindingWriteOptions { /* existing … */ pub engagement: Option<Arc<ActiveSet>> } // None => byte-identical code path
// report_finding: optional `asset { kind, coordinates }` input; route to profile_for_kind(kind) ->
//   validate that profile's completeness + declared classification systems -> upsert Asset -> record finding.
//   kind not owned by the active set => reject. No asset / None engagement => today's code path.
// asset_mark: set an Asset.depth from the owning profile's depth_ladder; MONOTONIC (clamp to deepest rung, return effective depth).
```

### Selection (launch paths)
- `rupu-cli`: `--engagement-profile <id>` / `--engagement-profiles a,b` on `run` + `session`; sub-agents inherit.
- `rupu-orchestrator`: workflow `defaults.engagement_profiles: [..]` + optional per-step **narrowing** (narrow-only). Composite expands.
- `rupu-agent`: frontmatter `engagementProfiles: [..]`; conflict with the active set at launch = hard error.
- `rupu-mcp`: `asset` arg on `findings.record`; validate against the run's active set; re-bless snapshot.
- Precedence: step → workflow → agent → built-in `code`.

---

# Part II — The full profile catalog (every profile, verbatim TOML)

> `code` is registered natively (its kinds map onto today's file/function ledger); the rest are `include_str!` TOML files under `crates/rupu-coverage/src/profile/builtin/`. No `[scope]` anywhere. Keys before the first `[[asset_kinds]]`.

### builtin/code.toml (native kinds, but authored as data for uniformity)
```toml
id = "code"
name = "Source code review"
evidence_blocks = ["text", "code_slice", "diff"]
classification_systems = ["CWE", "OWASP"]
[[asset_kinds]]
id = "repo"
coordinates = ["path"]
label = "{path}"
[[asset_kinds]]
id = "file"
parent = "repo"
coordinates = ["path", "line_range"]
label = "{path}:{line_range}"
[[asset_kinds]]
id = "function"
parent = "file"
coordinates = ["path", "symbol", "line_range"]
label = "{symbol} ({path})"
[[completeness]]
id = "located"
label = "Finding pinned to a path"
required = true
satisfied_when = { locator_has_coordinate = "path" }
[[completeness]]
id = "has_root_cause"
label = "Root cause stated"
required = true
satisfied_when = { has_field = "root_cause" }
[coverage]
enumerates = ["file"]
depth_ladder = ["unreviewed", "reviewed"]
```

### builtin/binary.toml
```toml
id = "binary"
name = "Binary reverse engineering"
evidence_blocks = ["text", "code_slice", "diff", "hexdump", "disasm"]
classification_systems = ["CWE", "CVE"]
[[asset_kinds]]
id = "binary"
coordinates = ["sha256"]
label = "{sha256}"
[[asset_kinds]]
id = "function"
parent = "binary"
coordinates = ["sha256", "address", "symbol"]
label = "{symbol} @ {address}"
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
```

### builtin/firmware.toml
```toml
id = "firmware"
name = "Firmware assessment"
evidence_blocks = ["text", "hexdump", "disasm", "decompile", "table"]
classification_systems = ["CWE", "CVE"]
[[asset_kinds]]
id = "image"
coordinates = ["sha256"]
label = "{sha256}"
[[asset_kinds]]
id = "partition"
parent = "image"
coordinates = ["sha256", "path"]
label = "{path}"
[[asset_kinds]]
id = "file"
parent = "partition"
coordinates = ["path", "sha256"]
label = "{path}"
[[asset_kinds]]
id = "function"
parent = "file"
coordinates = ["path", "address", "symbol"]
label = "{symbol} @ {address}"
[[completeness]]
id = "evidence_has_listing"
label = "Evidence includes a disassembly, decompile or hexdump"
required = true
satisfied_when = { any = [ { has_block_kind = "disasm" }, { has_block_kind = "decompile" }, { has_block_kind = "hexdump" } ] }
[[completeness]]
id = "classified"
label = "Weakness classified (CWE)"
required = true
satisfied_when = { has_classification_system = "CWE" }
[coverage]
enumerates = ["image", "partition", "file", "function"]
depth_ladder = ["acquired", "extracted", "unpacked", "analyzed"]
```

### builtin/network.toml
```toml
id = "network"
name = "Network security assessment"
evidence_blocks = ["text", "table", "scan_output", "http_exchange", "pcap_ref"]
classification_systems = ["CVE", "CWE", "CAPEC", "ATT&CK"]
[[asset_kinds]]
id = "host"
coordinates = ["host"]
label = "{host}"
[[asset_kinds]]
id = "service"
parent = "host"
coordinates = ["host", "port"]
label = "{host}:{port}"
[[completeness]]
id = "service_identified"
label = "Affected service pinned to host + port"
required = true
satisfied_when = { all = [ { locator_has_coordinate = "host" }, { locator_has_coordinate = "port" } ] }
[[completeness]]
id = "has_root_cause"
label = "Vulnerability / root cause stated"
required = true
satisfied_when = { has_field = "root_cause" }
[[completeness]]
id = "classified"
label = "Classified (CVE or CWE)"
required = true
satisfied_when = { any = [ { has_classification_system = "CVE" }, { has_classification_system = "CWE" } ] }
[coverage]
enumerates = ["host", "service"]
depth_ladder = ["discovered", "enumerated", "tested", "exploited"]
```

### builtin/web.toml
```toml
id = "web"
name = "Web application assessment"
evidence_blocks = ["text", "http_exchange", "image", "table"]
classification_systems = ["OWASP", "CWE", "CVE"]
[[asset_kinds]]
id = "site"
coordinates = ["url"]
label = "{url}"
[[asset_kinds]]
id = "route"
parent = "site"
coordinates = ["url", "http_route", "param"]
label = "{http_route}"
[[completeness]]
id = "route_identified"
label = "Finding pinned to a route"
required = true
satisfied_when = { any = [ { locator_has_coordinate = "http_route" }, { locator_has_coordinate = "url" } ] }
[[completeness]]
id = "evidence_has_exchange"
label = "Evidence includes an HTTP exchange"
required = true
satisfied_when = { has_block_kind = "http_exchange" }
[[completeness]]
id = "classified"
label = "Classified (OWASP or CWE)"
required = true
satisfied_when = { any = [ { has_classification_system = "OWASP" }, { has_classification_system = "CWE" } ] }
[coverage]
enumerates = ["site", "route"]
depth_ladder = ["mapped", "crawled", "tested", "exploited"]
```

### builtin/api.toml
```toml
id = "api"
name = "API security assessment"
evidence_blocks = ["text", "http_exchange", "table"]
classification_systems = ["OWASP-API", "CWE"]
[[asset_kinds]]
id = "service"
coordinates = ["url"]
label = "{url}"
[[asset_kinds]]
id = "endpoint"
parent = "service"
coordinates = ["url", "http_route", "param"]
label = "{http_route}"
[[completeness]]
id = "endpoint_identified"
label = "Finding pinned to an endpoint"
required = true
satisfied_when = { locator_has_coordinate = "http_route" }
[[completeness]]
id = "evidence_has_exchange"
label = "Evidence includes an HTTP exchange"
required = true
satisfied_when = { has_block_kind = "http_exchange" }
[coverage]
enumerates = ["service", "endpoint"]
depth_ladder = ["mapped", "tested", "exploited"]
```

### builtin/cloud.toml
```toml
id = "cloud"
name = "Cloud configuration review"
evidence_blocks = ["text", "table", "code_slice"]
classification_systems = ["CIS", "CWE", "ATT&CK"]
[[asset_kinds]]
id = "account"
coordinates = ["resource_id"]
label = "{resource_id}"
[[asset_kinds]]
id = "resource"
parent = "account"
coordinates = ["resource_id"]
label = "{resource_id}"
[[completeness]]
id = "resource_identified"
label = "Finding pinned to a resource"
required = true
satisfied_when = { locator_has_coordinate = "resource_id" }
[[completeness]]
id = "classified"
label = "Classified (CIS or CWE)"
required = true
satisfied_when = { any = [ { has_classification_system = "CIS" }, { has_classification_system = "CWE" } ] }
[coverage]
enumerates = ["account", "resource"]
depth_ladder = ["inventoried", "policy-evaluated", "validated"]
```

### builtin/sca.toml
```toml
id = "sca"
name = "Software composition analysis"
evidence_blocks = ["table", "text"]
classification_systems = ["CVE", "GHSA"]
[[asset_kinds]]
id = "repo"
coordinates = ["path"]
label = "{path}"
[[asset_kinds]]
id = "dependency"
parent = "repo"
coordinates = ["path", "resource_id"]
label = "{resource_id}"
[[completeness]]
id = "dependency_identified"
label = "Finding pinned to a dependency (purl)"
required = true
satisfied_when = { locator_has_coordinate = "resource_id" }
[[completeness]]
id = "classified"
label = "Classified (CVE or GHSA)"
required = true
satisfied_when = { any = [ { has_classification_system = "CVE" }, { has_classification_system = "GHSA" } ] }
[coverage]
enumerates = ["dependency"]
depth_ladder = ["inventoried", "resolved", "triaged"]
```

### builtin/iac.toml
```toml
id = "iac"
name = "Infrastructure-as-code review"
evidence_blocks = ["code_slice", "diff", "table"]
classification_systems = ["CIS", "CWE"]
[[asset_kinds]]
id = "repo"
coordinates = ["path"]
label = "{path}"
[[asset_kinds]]
id = "resource"
parent = "repo"
coordinates = ["path", "line_range"]
label = "{path}:{line_range}"
[[completeness]]
id = "located"
label = "Finding pinned to a file + lines"
required = true
satisfied_when = { all = [ { locator_has_coordinate = "path" }, { locator_has_coordinate = "line_range" } ] }
[[completeness]]
id = "classified"
label = "Classified (CIS or CWE)"
required = true
satisfied_when = { any = [ { has_classification_system = "CIS" }, { has_classification_system = "CWE" } ] }
[coverage]
enumerates = ["resource"]
depth_ladder = ["mapped", "reviewed"]
```

### builtin/secrets.toml
```toml
id = "secrets"
name = "Secret scanning"
evidence_blocks = ["code_slice", "text"]
classification_systems = ["CWE"]
[[asset_kinds]]
id = "repo"
coordinates = ["path"]
label = "{path}"
[[asset_kinds]]
id = "secret"
parent = "repo"
coordinates = ["path", "line_range", "commit"]
label = "{path}:{line_range}"
[[completeness]]
id = "located"
label = "Secret pinned to a file + lines"
required = true
satisfied_when = { all = [ { locator_has_coordinate = "path" }, { locator_has_coordinate = "line_range" } ] }
[[completeness]]
id = "classified"
label = "Classified (CWE)"
required = true
satisfied_when = { has_classification_system = "CWE" }
[coverage]
enumerates = ["secret"]
depth_ladder = ["scanned", "triaged", "confirmed"]
```

### builtin/container.toml
```toml
id = "container"
name = "Container image assessment"
evidence_blocks = ["table", "text"]
classification_systems = ["CVE"]
[[asset_kinds]]
id = "image"
coordinates = ["sha256"]
label = "{sha256}"
[[asset_kinds]]
id = "layer"
parent = "image"
coordinates = ["sha256", "path"]
label = "{path}"
[[asset_kinds]]
id = "package"
parent = "image"
coordinates = ["resource_id"]
label = "{resource_id}"
[[completeness]]
id = "component_identified"
label = "Finding pinned to a package or path"
required = true
satisfied_when = { any = [ { locator_has_coordinate = "resource_id" }, { locator_has_coordinate = "path" } ] }
[[completeness]]
id = "classified"
label = "Classified (CVE)"
required = true
satisfied_when = { has_classification_system = "CVE" }
[coverage]
enumerates = ["package"]
depth_ladder = ["inventoried", "scanned", "triaged"]
```

### builtin/redteam.toml
```toml
id = "redteam"
name = "Red team operation"
evidence_blocks = ["text", "scan_output", "image"]
classification_systems = ["ATT&CK"]
[[asset_kinds]]
id = "host"
coordinates = ["host"]
label = "{host}"
[[asset_kinds]]
id = "objective"
parent = "host"
coordinates = ["host", "resource_id"]
label = "{resource_id}"
[[completeness]]
id = "mapped_to_attack"
label = "Technique mapped to ATT&CK"
required = true
satisfied_when = { has_classification_system = "ATT&CK" }
[[completeness]]
id = "has_narrative"
label = "Execution narrative present"
required = true
satisfied_when = { has_field = "description" }
[coverage]
enumerates = ["host", "objective"]
depth_ladder = ["planned", "executed", "validated"]
```

### builtin/threat-model.toml (coordinate-less)
```toml
id = "threat-model"
name = "Threat model"
evidence_blocks = ["text", "image", "table"]
classification_systems = ["STRIDE", "CAPEC", "ATT&CK"]
[[asset_kinds]]
id = "system"
coordinates = []
label = "{name}"
[[asset_kinds]]
id = "component"
parent = "system"
coordinates = []
label = "{name}"
[[completeness]]
id = "has_threat"
label = "Threat described"
required = true
satisfied_when = { has_field = "description" }
[[completeness]]
id = "classified"
label = "Classified (STRIDE)"
required = true
satisfied_when = { has_classification_system = "STRIDE" }
[coverage]
enumerates = ["component"]
depth_ladder = ["modeled", "reviewed"]
```

### builtin/mobile.toml (composite + own root + MASVS)
```toml
id = "mobile"
name = "Mobile application assessment"
includes = ["binary", "web"]
evidence_blocks = ["disasm", "http_exchange", "image"]
classification_systems = ["MASVS", "OWASP", "CWE"]
[[asset_kinds]]
id = "package"
coordinates = ["sha256"]
label = "{sha256}"
[[completeness]]
id = "package_identified"
label = "Finding pinned to a package, function or route"
required = true
satisfied_when = { any = [ { locator_has_coordinate = "sha256" }, { locator_has_coordinate = "address" }, { locator_has_coordinate = "http_route" } ] }
[coverage]
enumerates = ["package"]
depth_ladder = ["unpacked", "static", "dynamic"]
```

### builtin/pentest.toml (pure composite)
```toml
id = "pentest"
name = "Penetration test"
includes = ["network", "web"]
```

---

# Part III — Execution (single pass, in order)

Each step ends green (`cargo test -p <touched crates>` + clippy `-D warnings` + per-file rustfmt) and a local commit.

1. **Clean slate.** New branch off `main`. `git revert` the #687 squash merge (`a075ed8d`) so the tree has zero engagement code (verify: no `profile/`, `asset/`, `scope/`, no `Coordinate`/profile-aware `report_finding`, no `--engagement-profile*`). Clean the spec (remove scope/RoE per the OUT-of-scope section; re-root network→host, web→site). `git rm` the three fragmented plan docs. Commit.
2. **Coordinate + Locator** (Part I) + tests. Commit.
3. **Asset + asset store** + tests. Commit.
4. **Evidence blocks + renderers + Classification** (+ schema/validator/DTO lockstep, back-compat) + tests. Re-bless MCP snapshot if needed. Commit.
5. **Predicate + CompletenessCheck** + tests. Commit.
6. **Profile package + loader + registry + routing** + tests. Commit.
7. **report_finding profile-aware + asset_mark** + tests (incl. byte-identical no-engagement path). Commit.
8. **Selection** on cli/workflow/agent/mcp + precedence + tests. Commit.
9. **Author & register the full catalog** (Part II: code, binary, firmware, network, web, api, cloud, sca, iac, secrets, container, redteam, threat-model, mobile, pentest). Each loads under the strict schema, routes, and a representative finding's completeness evaluates. `builtin_profiles` key-set test lists all 15. Commit per group.
10. **End-to-end + docs.** Mock-provider e2e for binary + network + a `pentest` run filing `network:service` and `web:route` side by side. Docs (`coverage.md`, `agent-format.md`, `workflow-format.md`) — profiles-only, no scope language. Commit.
11. **Finish.** Grep the tree: `enforce_in_scope`/`ScopePolicy`/`cidrs`/`[scope]`/`sandbox`/`http_request` → zero hits. Whole-branch review. One push → fresh PR → release-beta flow.

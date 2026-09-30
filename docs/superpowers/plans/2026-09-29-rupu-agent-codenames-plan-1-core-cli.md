# Agent Codenames — Plan 1 (Core + CLI) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Every workflow run, standalone agent run, session, fan-out unit and sub-agent gets a stored human codename (`cobalt-harbor/heron#412>lynx#3`), carried on records, transcripts and executor events (plus a new `AgentStarted` event with agent · provider · model), and shown/accepted by the CLI.

**Architecture:** A new dependency-free leaf crate `rupu-codename` owns word lists, hashing, the `Codename` grammar, visual identity (tint/badge) and the `CrewNamer` allocator. The orchestrator builds a `RunNaming` per workflow run (static-slot walk + persisted dynamic state) and mints a codename next to every `run_id` it mints; the agent runtime writes it into `RunStart`, the system prompt and `ToolContext`; the CLI's sub-agent dispatcher allocates `>role#n` children. The CLI renders names and resolves them back to ids.

**Tech Stack:** Rust 2021 workspace, serde/serde_json, tokio, clap, owo-colors (existing), insta/assert_fs (existing test deps).

**Spec:** `docs/superpowers/specs/2026-09-29-rupu-agent-codenames-design.md`

## Global Constraints

- Workspace deps only: versions live in root `Cargo.toml`; crate `Cargo.toml`s use `{ workspace = true }`. New crate must be added to the explicit `[workspace] members` list.
- `#![deny(clippy::all)]` workspace-wide, `unsafe_code` forbidden, `disallowed_methods = "deny"` — every task ends clippy-clean for touched crates.
- Every new serde field is `#[serde(default, skip_serializing_if = "Option::is_none")]` (or `Vec::is_empty`) so every legacy file round-trips byte-for-byte.
- Hashing is FNV-1a 64 over the **whole** id string — never `std::collections::hash_map::DefaultHasher`.
- Word lists are FROZEN once merged (golden tests pin them).
- Names are minted once and stored; nothing recomputes a stored name. Only `derive_legacy` recomputes, for records without one.
- macOS app (`apps/rupu-macos`) is out of scope; do not touch Swift. `rupu-cp` is touched only where a struct literal must gain a field (`None`) or where Task 8 says so — CP display is Plan 2.
- **Formatting:** main is fmt-dirty under the pinned toolchain. NEVER run package-wide `cargo fmt`; run `rustfmt --edition 2021 <file>` on files you changed only.
- **Toolchain:** worktrees have no rustup; measure the baseline yourself (`cargo test -p <crate>` before your change) rather than assuming red/green.
- **Git:** never `git stash` / `git stash pop` (the stash stack is shared with other sessions). Commit per task on branch `claude/agent-identity-codenames-3c19a5`. End commit messages with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Struct literals: `RunRecord`, `AgentRunOpts`, `ToolContext`, `StepResultRecord`, `UnitCheckpoint`, `Attribution`, `AgentLaunchRequest` have **no `Default`** (or tests build them literally). Adding a field breaks every literal, including tests in other crates. Use `cargo build --workspace --tests 2>&1 | rg 'missing field'` to find them all and add the field (`None` unless the task says otherwise).

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/rupu-codename/Cargo.toml` | new crate manifest |
| `crates/rupu-codename/src/lib.rs` | public API: `crew_for`, `role_word`, `derive_legacy`, re-exports |
| `crates/rupu-codename/src/hash.rs` | `fnv1a64` |
| `crates/rupu-codename/src/words.rs` | `NOUNS`, `ROLES` (frozen) |
| `crates/rupu-codename/src/palette.rs` | `COLORS`, `Tint`, `Shape`, `Badge`, `crew_tint`, `role_badge` |
| `crates/rupu-codename/src/codename.rs` | `Codename`/`Segment` grammar, Display/FromStr/serde |
| `crates/rupu-codename/src/namer.rs` | `CrewNamer` allocator + `SharedNamer` (Arc<Mutex>, JSON persistence) |
| `crates/rupu-orchestrator/src/codenames.rs` | `RunNaming`: static-slot walk over a `Workflow`, per-site codename constructors |
| `crates/rupu-cli/src/output/codename.rs` | CLI rendering helpers (badge glyph + tint) and crew resolution |

Existing files modified are named per task.

---

### Task 1: `rupu-codename` crate — words, hashing, grammar, palette

**Files:**
- Create: `crates/rupu-codename/Cargo.toml`, `src/lib.rs`, `src/hash.rs`, `src/words.rs`, `src/palette.rs`, `src/codename.rs`
- Modify: `Cargo.toml` (root: `members` list; `[workspace.dependencies]` add `rupu-codename = { path = "crates/rupu-codename" }` next to the existing `rupu-netflow` path entry)

**Interfaces:**
- Produces:
  - `pub fn crew_for(id: &str) -> String` — `"<color>-<noun>"`
  - `pub fn role_word(agent_def: &str) -> &'static str` — base word, no collision handling
  - `pub fn derive_legacy(id: &str, agent: Option<&str>) -> String` — `"crew"` or `"crew/role"`
  - `pub struct Codename { pub crew: String, pub segments: Vec<Segment> }` with `Codename::crew_only(impl Into<String>)`, `child(&self, role: &str, n: Option<u32>) -> Codename`, `with_attempt(self, u32) -> Codename`, `leaf(&self) -> String`, `Display`, `FromStr<Err = ParseCodenameError>`, `Serialize`/`Deserialize` as string
  - `pub struct Segment { pub role: String, pub n: Option<u32>, pub attempt: Option<u32> }`
  - `pub struct Tint { pub light: &'static str, pub dark: &'static str }` + `fn dark_rgb(&self) -> (u8, u8, u8)`
  - `pub fn crew_tint(crew: &str) -> Option<Tint>`
  - `pub enum Shape { Circle, Triangle, Square, Diamond, Pentagon, Hexagon, Star, Cross, Ring, Chevron, InvTriangle, Half }` + `fn glyph(self) -> char`
  - `pub struct Badge { pub shape: Shape, pub tint: Tint }`, `pub fn role_badge(role: &str) -> Badge`
  - `pub(crate) fn fnv1a64(s: &str) -> u64`
  - `pub use words::{NOUNS, ROLES}; pub use palette::COLORS;`

- [ ] **Step 1: Manifest + workspace wiring**

`crates/rupu-codename/Cargo.toml`:
```toml
[package]
name = "rupu-codename"
version.workspace = true
edition.workspace = true

[lints]
workspace = true

[dependencies]
serde = { workspace = true, features = ["derive"] }
serde_json = { workspace = true }
thiserror = { workspace = true }
tracing = { workspace = true }

[dev-dependencies]
tempfile = { workspace = true }
```
Root `Cargo.toml`: add `"crates/rupu-codename",` to `[workspace] members` (keep alphabetical order with neighbours) and `rupu-codename = { path = "crates/rupu-codename" }` under `[workspace.dependencies]` beside `rupu-netflow`.

- [ ] **Step 2: Word lists and colors (verbatim — these are frozen)**

`src/words.rs`:
```rust
//! Word lists. FROZEN once shipped: `crew_for`/`role_word` index into
//! these by hash, so reordering, inserting or removing a word renames every
//! derive-on-read (legacy) record. `tests::golden_*` pins them.

/// Crew nouns: places and objects. Never animals (reserved for roles) or colors.
pub const NOUNS: &[&str] = &[
    "anchor", "anvil", "arbor", "arch", "atlas", "aurora", "basin", "bastion", "bay",
    "beacon", "bell", "bluff", "bough", "bridge", "brook", "butte", "cairn", "canal",
    "canopy", "canyon", "cape", "cask", "castle", "cavern", "cellar", "chapel", "citadel",
    "cliff", "cloud", "coast", "comet", "compass", "cove", "crag", "crater", "creek",
    "crest", "crown", "delta", "dune", "ember", "estuary", "fen", "ferry", "field", "fjord",
    "flint", "forge", "fort", "fountain", "gable", "garden", "geyser", "glacier", "glade",
    "glen", "gorge", "grove", "gulf", "harbor", "haven", "hearth", "heath", "helm", "hill",
    "hollow", "horizon", "isle", "jetty", "keep", "kettle", "kiln", "knoll", "lagoon",
    "lake", "lantern", "ledge", "lens", "loom", "manor", "marsh", "meadow", "mesa", "mill",
    "mirror", "moor", "moraine", "nebula", "needle", "oasis", "orbit", "orchard", "outpost",
    "palace", "pass", "pavilion", "peak", "pier", "pillar", "pine", "plain", "plateau",
    "pond", "portal", "prairie", "prism", "quarry", "quay", "rampart", "rapids", "reef",
    "ridge", "river", "road", "rock", "rudder", "sail", "savanna", "shoal", "shore",
    "sierra", "signal", "sky", "slope", "spire", "spring", "spur", "star", "steppe",
    "stone", "strait", "summit", "sundial", "tarn", "temple", "terrace", "thicket", "tide",
    "timber", "tower", "trail", "tundra", "valley", "vault", "vessel", "vista", "volcano",
    "wharf", "willow", "wind", "atoll", "barrow", "bayou", "beach", "bramble", "brink",
    "cascade", "causeway", "chasm", "cinder", "cistern", "cobble", "corridor", "crossing",
    "dell", "dike", "dockyard", "fathom", "foundry", "furnace", "garret", "gate", "grotto",
    "hamlet", "hangar", "hedge", "inlet", "island", "kingdom", "ladder", "landing",
    "lattice", "lodge", "lookout", "marina", "meridian", "mound", "narrows", "nexus",
    "paddock", "parapet", "pasture", "pergola", "pinnacle", "quarter", "rain", "range",
    "refuge", "relay", "runway", "shelter", "sluice", "sound", "spindle", "stack",
    "steeple", "stream", "tablet", "thatch", "tunnel", "turret", "upland", "village",
    "vineyard", "wall", "well", "wood", "yard",
];

/// Role words: animals. Never colors or crew nouns.
pub const ROLES: &[&str] = &[
    "adder", "alpaca", "anole", "ant", "antelope", "ape", "asp", "auk", "avocet", "badger",
    "barbel", "bat", "bear", "beaver", "bee", "beetle", "bison", "bittern", "boar",
    "bobcat", "bonobo", "bream", "buffalo", "bunting", "buzzard", "caiman", "camel",
    "canary", "caracal", "cardinal", "caribou", "carp", "cat", "catfish", "chamois",
    "cheetah", "chough", "cicada", "civet", "clam", "cobra", "cod", "condor", "coot",
    "cougar", "coyote", "crab", "crane", "cricket", "crow", "cuckoo", "curlew", "dace",
    "deer", "dingo", "dipper", "dodo", "dolphin", "donkey", "dove", "drake", "dunlin",
    "eagle", "egret", "eider", "eland", "elk", "emu", "ermine", "falcon", "ferret", "finch",
    "firefly", "flamingo", "fox", "frog", "gannet", "gar", "gazelle", "gecko", "gerbil",
    "gibbon", "gnu", "goat", "godwit", "goose", "gopher", "gorilla", "goshawk", "grebe",
    "grouse", "gull", "guppy", "hare", "harrier", "hawk", "hedgehog", "heron", "hippo",
    "hoopoe", "hornet", "horse", "hyena", "ibex", "ibis", "iguana", "impala", "jackal",
    "jaguar", "jay", "kestrel", "kite", "kiwi", "koala", "krill", "kudu", "lark", "lemming",
    "lemur", "leopard", "limpet", "linnet", "lion", "llama", "lobster", "locust", "loon",
    "lory", "lynx", "macaw", "magpie", "mallard", "mamba", "manatee", "mandrill", "mantis",
    "marlin", "marmot", "marten", "merlin", "mink", "minnow", "mole", "mongoose", "moose",
    "moth", "mouse", "mule", "murre", "narwhal", "newt", "numbat", "ocelot", "octopus",
    "okapi", "oriole", "oryx", "osprey", "ostrich", "otter", "owl", "ox", "oyster", "panda",
    "panther", "parrot", "peacock", "pelican", "penguin", "perch", "petrel", "pheasant",
    "pika", "pike", "pipit", "plover", "polecat", "pony", "porpoise", "possum", "prawn",
    "puffin", "puma", "python", "quail", "quokka", "quoll", "rabbit", "raccoon", "rail",
    "ram", "raven", "ray", "redstart", "reindeer", "rhea", "rhino", "robin", "rook",
    "sable", "salmon", "sardine", "scarab", "seal", "serval", "shark", "shrew", "shrike",
    "shrimp", "siskin", "skate", "skink", "skua", "skunk", "sloth", "smelt", "snail",
    "snipe", "sparrow", "spider", "squid", "stag", "starling", "stoat", "stork", "sturgeon",
    "sunbird", "swallow", "swan", "swift", "tahr", "tamarin", "tapir", "tarpon", "tern",
    "termite", "thrush", "tiger", "toad", "tortoise", "toucan", "trout", "tuna", "turtle",
    "urchin", "vicuna", "viper", "vole", "vulture", "wallaby", "walrus", "warbler", "wasp",
    "weasel", "whale", "whelk", "wolf", "wombat", "wren", "yak", "zebra",
];
```

`src/palette.rs` (top of file):
```rust
/// Crew colors: `(name, light-theme hex, dark-theme hex)`. Both hexes clear
/// 3:1 non-text contrast against `#ffffff` / `#0f1115` (`tests::palette_contrast`).
/// Pure status red/green are deliberately absent.
pub const COLORS: &[(&str, &str, &str)] = &[
    ("amber", "#b45309", "#fbbf24"),
    ("apricot", "#c2410c", "#fdba74"),
    ("azure", "#0369a1", "#7dd3fc"),
    ("bronze", "#92400e", "#d6a064"),
    ("cedar", "#7c4a2d", "#c8906a"),
    ("cerise", "#be185d", "#f472b6"),
    ("cobalt", "#1d4ed8", "#93b4fd"),
    ("copper", "#9a3412", "#f0a070"),
    ("coral", "#c2412d", "#fb8f78"),
    ("cyan", "#0e7490", "#67e8f9"),
    ("denim", "#1e40af", "#8fa8e8"),
    ("ebony", "#3f3f46", "#a1a1aa"),
    ("fawn", "#8a6a4a", "#d8b894"),
    ("fuchsia", "#a21caf", "#f0abfc"),
    ("ginger", "#b45f06", "#f5a55a"),
    ("gold", "#a16207", "#fde047"),
    ("graphite", "#52525b", "#d4d4d8"),
    ("hazel", "#7a5c2e", "#c9a66b"),
    ("indigo", "#4338ca", "#a5b4fc"),
    ("iris", "#5b4bc4", "#b4a9f5"),
    ("jade", "#0f766e", "#5eead4"),
    ("khaki", "#6b6a2a", "#d0cc8a"),
    ("lapis", "#1e3a8a", "#8ea6f0"),
    ("lavender", "#7c5cc4", "#c4b5fd"),
    ("lilac", "#9061a8", "#dcb8f0"),
    ("magenta", "#a3137a", "#f58ad8"),
    ("mauve", "#8e5a7a", "#d9a5c6"),
    ("mint", "#0f7a5a", "#86efcf"),
    ("ochre", "#9a6b0a", "#e6b34a"),
    ("olive", "#5c6b1f", "#bccb6a"),
    ("onyx", "#27272a", "#b8b8c0"),
    ("orchid", "#9d3fa8", "#e9a0f0"),
    ("pearl", "#6e7380", "#e6e8ee"),
    ("peach", "#c2562a", "#fdba9a"),
    ("pewter", "#5f6b73", "#bcc6cc"),
    ("plum", "#7e2a6e", "#e0a0d6"),
    ("rust", "#9a3a12", "#e88a5a"),
    ("saffron", "#b7791f", "#fcd05a"),
    ("sage", "#56705a", "#b5ccb0"),
    ("sand", "#8a7350", "#e2cfa8"),
    ("sepia", "#704214", "#c9a07a"),
    ("sienna", "#a0522d", "#e3a07a"),
    ("silver", "#6b7280", "#d1d5db"),
    ("slate", "#475569", "#cbd5e1"),
    ("steel", "#4a6078", "#a8bdd0"),
    ("tan", "#8a6440", "#d8b48c"),
    ("teal", "#0d7377", "#5fd4d8"),
    ("topaz", "#b8860b", "#f7d070"),
    ("umber", "#6b4423", "#c49a78"),
    ("violet", "#6d28d9", "#c4b5fd"),
    ("wine", "#7a1f3d", "#e38aa8"),
];
```

- [ ] **Step 3: Write the failing tests**

`src/lib.rs` (tests module at the bottom; the non-test body comes in Step 5):
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn golden_crews() {
        // Pinned: a change here means the word lists or hash changed, which
        // renames every legacy (derive-on-read) record. Don't "fix" by updating.
        assert_eq!(crew_for("run_01J9ZQ3K4M5N6P7Q8R9S0T1V2W"), "jade-reef");
        assert_eq!(crew_for("run_01J9ZQ3K4M5N6P7Q8R9S0T1V2X"), "peach-prairie");
        assert_eq!(crew_for("ses_01J9ZQ3K4M5N6P7Q8R9S0T1V2W"), "olive-pine");
    }

    #[test]
    fn golden_roles() {
        assert_eq!(role_word("security-reviewer"), "ferret");
        assert_eq!(role_word("triage"), "numbat");
        assert_eq!(role_word("ag"), "hedgehog");
    }

    #[test]
    fn whole_id_is_hashed() {
        // Same-millisecond ULIDs share their first 10 chars; only the tail differs.
        assert_ne!(
            crew_for("run_01J9ZQ3K4M5N6P7Q8R9S0T1V2W"),
            crew_for("run_01J9ZQ3K4M5N6P7Q8R9S0T1V2X")
        );
    }

    #[test]
    fn word_list_hygiene() {
        use std::collections::HashSet;
        let colors: Vec<&str> = COLORS.iter().map(|c| c.0).collect();
        for (name, list, min) in [("colors", &colors[..], 48), ("nouns", NOUNS, 200), ("roles", ROLES, 250)] {
            assert!(list.len() >= min, "{name}: {} < {min}", list.len());
            let set: HashSet<_> = list.iter().collect();
            assert_eq!(set.len(), list.len(), "{name} has duplicates");
            for w in list {
                assert!(w.len() <= 8 && w.chars().all(|c| c.is_ascii_lowercase()), "{name}: bad word {w:?}");
            }
        }
        let c: HashSet<_> = colors.iter().copied().collect();
        let n: HashSet<_> = NOUNS.iter().copied().collect();
        let r: HashSet<_> = ROLES.iter().copied().collect();
        assert!(c.is_disjoint(&n) && c.is_disjoint(&r) && n.is_disjoint(&r));
    }

    #[test]
    fn derive_legacy_shapes() {
        assert_eq!(derive_legacy("run_01J9ZQ3K4M5N6P7Q8R9S0T1V2W", None), "jade-reef");
        assert_eq!(
            derive_legacy("run_01J9ZQ3K4M5N6P7Q8R9S0T1V2W", Some("triage")),
            "jade-reef/numbat"
        );
    }
}
```

`src/codename.rs` tests:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_every_shape() {
        for s in [
            "cobalt-harbor",
            "cobalt-harbor/heron",
            "cobalt-harbor/heron#412",
            "cobalt-harbor/heron#412.2",
            "cobalt-harbor/heron#412>lynx#3",
            "cobalt-harbor/heron#412>lynx#3>otter#1",
            "saffron-ridge/heron>lynx#5",
        ] {
            let c: Codename = s.parse().unwrap();
            assert_eq!(c.to_string(), s);
        }
    }

    #[test]
    fn leaf_drops_the_crew() {
        let c: Codename = "cobalt-harbor/heron#412>lynx#3".parse().unwrap();
        assert_eq!(c.leaf(), "heron#412>lynx#3");
        assert_eq!(Codename::crew_only("cobalt-harbor").leaf(), "cobalt-harbor");
    }

    #[test]
    fn builders() {
        let c = Codename::crew_only("cobalt-harbor").child("heron", Some(412)).with_attempt(2);
        assert_eq!(c.to_string(), "cobalt-harbor/heron#412.2");
        assert_eq!(c.child("lynx", Some(1)).to_string(), "cobalt-harbor/heron#412.2>lynx#1");
    }

    #[test]
    fn rejects_non_codenames() {
        for s in ["run_01J9ZQ", "01J9ZQ3K", "cobalt", "Cobalt-harbor", "cobalt-harbor/", "cobalt-harbor/heron#", "cobalt-harbor/heron#0", "a-b/c>>d"] {
            assert!(s.parse::<Codename>().is_err(), "{s} should not parse");
        }
    }

    #[test]
    fn serde_is_the_string_form() {
        let c: Codename = "cobalt-harbor/heron#4".parse().unwrap();
        assert_eq!(serde_json::to_string(&c).unwrap(), "\"cobalt-harbor/heron#4\"");
        let back: Codename = serde_json::from_str("\"cobalt-harbor/heron#4\"").unwrap();
        assert_eq!(back, c);
    }
}
```

`src/palette.rs` tests:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn lum(hex: &str) -> f64 {
        let ch = |i: usize| {
            let c = u8::from_str_radix(&hex[i..i + 2], 16).unwrap() as f64 / 255.0;
            if c <= 0.03928 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * ch(1) + 0.7152 * ch(3) + 0.0722 * ch(5)
    }
    fn contrast(a: &str, b: &str) -> f64 {
        let (x, y) = (lum(a), lum(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    #[test]
    fn palette_contrast() {
        for (name, light, dark) in COLORS {
            assert!(contrast(light, "#ffffff") >= 3.0, "{name} light too faint");
            assert!(contrast(dark, "#0f1115") >= 3.0, "{name} dark too faint");
        }
    }

    #[test]
    fn crew_tint_uses_the_color_word() {
        let t = crew_tint("cobalt-harbor").unwrap();
        assert_eq!(t.light, "#1d4ed8");
        assert_eq!(t.dark_rgb(), (0x93, 0xb4, 0xfd));
        assert!(crew_tint("nope-harbor").is_none());
    }

    #[test]
    fn badges_are_stable_and_varied() {
        assert_eq!(role_badge("heron"), role_badge("heron"));
        let distinct: std::collections::HashSet<_> =
            crate::ROLES.iter().map(|r| { let b = role_badge(r); (b.shape, b.tint.light) }).collect();
        assert!(distinct.len() >= 100, "only {} distinct badges", distinct.len());
    }
}
```

- [ ] **Step 4: Run tests to verify they fail**

Run: `cargo test -p rupu-codename`
Expected: compile errors (`crew_for`, `Codename`, `role_badge` … not found).

- [ ] **Step 5: Implement**

`src/hash.rs`:
```rust
/// FNV-1a 64. Stable across Rust versions and platforms, unlike
/// `DefaultHasher` — codenames are persisted, so the hash must never drift.
pub(crate) fn fnv1a64(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}
```

`src/palette.rs` (below `COLORS`):
```rust
use crate::hash::fnv1a64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Tint {
    pub light: &'static str,
    pub dark: &'static str,
}

impl Tint {
    /// Terminal rendering uses the dark-theme hex (terminals are overwhelmingly dark).
    pub fn dark_rgb(&self) -> (u8, u8, u8) {
        let p = |i: usize| u8::from_str_radix(&self.dark[i..i + 2], 16).unwrap_or(0);
        (p(1), p(3), p(5))
    }
}

fn tint_named(name: &str) -> Option<Tint> {
    COLORS
        .iter()
        .find(|(n, _, _)| *n == name)
        .map(|(_, light, dark)| Tint { light, dark })
}

/// The crew's tint is its color word: `cobalt-harbor` → cobalt.
pub fn crew_tint(crew: &str) -> Option<Tint> {
    tint_named(crew.split('-').next()?)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Shape {
    Circle, Triangle, Square, Diamond, Pentagon, Hexagon,
    Star, Cross, Ring, Chevron, InvTriangle, Half,
}

const SHAPES: [Shape; 12] = [
    Shape::Circle, Shape::Triangle, Shape::Square, Shape::Diamond, Shape::Pentagon, Shape::Hexagon,
    Shape::Star, Shape::Cross, Shape::Ring, Shape::Chevron, Shape::InvTriangle, Shape::Half,
];

impl Shape {
    /// Single-column glyph for terminal output.
    pub fn glyph(self) -> char {
        match self {
            Shape::Circle => '●', Shape::Triangle => '▲', Shape::Square => '■',
            Shape::Diamond => '◆', Shape::Pentagon => '⬟', Shape::Hexagon => '⬢',
            Shape::Star => '★', Shape::Cross => '✚', Shape::Ring => '◯',
            Shape::Chevron => '❯', Shape::InvTriangle => '▼', Shape::Half => '◐',
        }
    }
}

/// Badge hues: a spread subset of `COLORS`, so badges and crew tints share one palette.
const BADGE_HUES: [&str; 12] = [
    "cobalt", "jade", "amber", "fuchsia", "cyan", "violet",
    "ochre", "cerise", "teal", "indigo", "copper", "olive",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Badge {
    pub shape: Shape,
    pub tint: Tint,
}

/// Stable per role word: shape from the hash, hue from the hash's next digit.
pub fn role_badge(role: &str) -> Badge {
    let h = fnv1a64(role);
    let shape = SHAPES[(h % 12) as usize];
    let hue = BADGE_HUES[((h / 12) % 12) as usize];
    Badge { shape, tint: tint_named(hue).unwrap_or(Tint { light: "#6b7280", dark: "#d1d5db" }) }
}
```

`src/codename.rs`:
```rust
use std::fmt;
use std::str::FromStr;

/// One agent in the path: `role[#n][.attempt]`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Segment {
    pub role: String,
    /// Instance number, 1-based. `None` for a statically-known singleton.
    pub n: Option<u32>,
    /// Retry attempt, ≥ 2. `None` on the first attempt.
    pub attempt: Option<u32>,
}

/// `crew[/role[#n][.a](>role[#n][.a])*]` — see the codenames spec §3.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Codename {
    pub crew: String,
    pub segments: Vec<Segment>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("not a codename: {0:?}")]
pub struct ParseCodenameError(pub String);

impl Codename {
    pub fn crew_only(crew: impl Into<String>) -> Self {
        Self { crew: crew.into(), segments: Vec::new() }
    }

    /// A child agent: the first call adds the member (`crew/role`), later
    /// calls add a sub-agent (`…>role`).
    pub fn child(&self, role: &str, n: Option<u32>) -> Self {
        let mut c = self.clone();
        c.segments.push(Segment { role: role.to_string(), n, attempt: None });
        c
    }

    /// Mark the last segment as retry attempt `attempt` (≥ 2; 1 is a no-op).
    pub fn with_attempt(mut self, attempt: u32) -> Self {
        if attempt >= 2 {
            if let Some(last) = self.segments.last_mut() {
                last.attempt = Some(attempt);
            }
        }
        self
    }

    /// Everything after the crew (`heron#412>lynx#3`), or the crew itself when
    /// there are no segments.
    pub fn leaf(&self) -> String {
        if self.segments.is_empty() {
            return self.crew.clone();
        }
        self.segments.iter().map(Segment::to_string).collect::<Vec<_>>().join(">")
    }
}

impl fmt::Display for Segment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.role)?;
        if let Some(n) = self.n {
            write!(f, "#{n}")?;
        }
        if let Some(a) = self.attempt {
            write!(f, ".{a}")?;
        }
        Ok(())
    }
}

impl fmt::Display for Codename {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.crew)?;
        if !self.segments.is_empty() {
            write!(f, "/{}", self.leaf())?;
        }
        Ok(())
    }
}

fn is_word(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
}

fn positive(s: &str) -> Option<u32> {
    s.parse::<u32>().ok().filter(|n| *n >= 1)
}

fn parse_segment(s: &str) -> Option<Segment> {
    let (head, attempt) = match s.split_once('.') {
        Some((h, a)) => (h, Some(positive(a).filter(|a| *a >= 2)?)),
        None => (s, None),
    };
    let (role, n) = match head.split_once('#') {
        Some((r, n)) => (r, Some(positive(n)?)),
        None => (head, None),
    };
    is_word(role).then(|| Segment { role: role.to_string(), n, attempt })
}

impl FromStr for Codename {
    type Err = ParseCodenameError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || ParseCodenameError(s.to_string());
        let (crew, rest) = match s.split_once('/') {
            Some((c, r)) => (c, Some(r)),
            None => (s, None),
        };
        let (color, noun) = crew.split_once('-').ok_or_else(err)?;
        if !is_word(color) || !is_word(noun) {
            return Err(err());
        }
        let segments = match rest {
            None => Vec::new(),
            Some(r) => r.split('>').map(parse_segment).collect::<Option<Vec<_>>>().ok_or_else(err)?,
        };
        if rest.is_some() && segments.is_empty() {
            return Err(err());
        }
        Ok(Self { crew: crew.to_string(), segments })
    }
}

impl serde::Serialize for Codename {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> serde::Deserialize<'de> for Codename {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}
```
(`"cobalt-harbor/"` fails because `"".split('>')` yields one empty segment which `is_word` rejects.)

`src/lib.rs`:
```rust
//! Human codenames for rupu runs, sessions and agent instances —
//! `cobalt-harbor/heron#412>lynx#3`. Leaf crate: no rupu deps.
//! Spec: docs/superpowers/specs/2026-09-29-rupu-agent-codenames-design.md

mod codename;
mod hash;
mod palette;
mod words;

pub use codename::{Codename, ParseCodenameError, Segment};
pub use palette::{crew_tint, role_badge, Badge, Shape, Tint, COLORS};
pub use words::{NOUNS, ROLES};

use hash::fnv1a64;

/// Crew name for a top-level id (workflow run, standalone run, session).
pub fn crew_for(id: &str) -> String {
    let h = fnv1a64(id);
    let c = COLORS.len() as u64;
    let color = COLORS[(h % c) as usize].0;
    let noun = NOUNS[((h / c) % NOUNS.len() as u64) as usize];
    format!("{color}-{noun}")
}

/// Base role word for an agent definition. No collision handling — use
/// [`CrewNamer`] when minting; this is for singletons and legacy derivation.
pub fn role_word(agent_def: &str) -> &'static str {
    ROLES[(fnv1a64(agent_def) % ROLES.len() as u64) as usize]
}

/// Name for a record written before codenames existed. Only `rupu-cp` DTOs
/// and CLI listings call this; it is never stored.
pub fn derive_legacy(id: &str, agent: Option<&str>) -> String {
    let crew = Codename::crew_only(crew_for(id));
    match agent {
        Some(a) => crew.child(role_word(a), None).to_string(),
        None => crew.to_string(),
    }
}
```
(`mod namer;` and `pub use namer::{CrewNamer, SharedNamer};` are added in Task 2.)

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p rupu-codename && cargo clippy -p rupu-codename --all-targets -- -D warnings`
Expected: all PASS, clippy clean. If `palette_contrast` fails for a color, darken its light hex / lighten its dark hex (keep the name) until it passes.

- [ ] **Step 7: Commit**

```bash
rustfmt --edition 2021 crates/rupu-codename/src/*.rs
git add Cargo.toml Cargo.lock crates/rupu-codename
git commit -m "feat(codename): rupu-codename crate — words, grammar, palette, badges"
```

---

### Task 2: `CrewNamer` allocator + `SharedNamer` persistence

**Files:**
- Create: `crates/rupu-codename/src/namer.rs`
- Modify: `crates/rupu-codename/src/lib.rs` (add `mod namer;` + `pub use namer::{CrewNamer, SharedNamer};`)

**Interfaces:**
- Consumes: `fnv1a64`, `ROLES`, `Codename` (Task 1)
- Produces:
  - `#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)] pub struct CrewNamer { pub crew: String, roles: BTreeMap<String, Vec<String>>, counters: BTreeMap<String, u32> }`
  - `CrewNamer::new(crew: impl Into<String>) -> Self`
  - `fn allocate_role(&mut self, agent_def: &str) -> String` — a NEW word every call (static slots)
  - `fn canonical_role(&mut self, agent_def: &str) -> String` — the def's first word, allocating if absent
  - `fn seed_role(&mut self, agent_def: &str, word: &str)` — record an externally-decided word as canonical
  - `fn next_instance(&mut self, parent: &Codename, role: &str) -> u32` — 1-based, per (parent, role)
  - `#[derive(Debug, Clone)] pub struct SharedNamer` with `in_memory(CrewNamer) -> Self`, `open_or_init(path: PathBuf, init: impl FnOnce() -> CrewNamer) -> Self`, `with<R>(&self, f: impl FnOnce(&mut CrewNamer) -> R) -> R`, `crew(&self) -> String`

- [ ] **Step 1: Write the failing tests** (bottom of `namer.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_def_second_slot_gets_next_free_word() {
        let mut n = CrewNamer::new("jade-reef");
        assert_eq!(n.allocate_role("ag"), "hedgehog");
        assert_eq!(n.allocate_role("ag"), "heron"); // ROLES order: hedgehog, heron
        assert_eq!(n.canonical_role("ag"), "hedgehog");
    }

    #[test]
    fn different_defs_colliding_on_a_word_are_probed() {
        let mut n = CrewNamer::new("jade-reef");
        n.seed_role("other", "ferret");
        // security-reviewer's base word is ferret; it's taken, so it probes.
        assert_ne!(n.canonical_role("security-reviewer"), "ferret");
    }

    #[test]
    fn instance_counters_are_per_parent_and_role() {
        let mut n = CrewNamer::new("jade-reef");
        let a: Codename = "jade-reef/heron#1".parse().unwrap();
        let b: Codename = "jade-reef/heron#2".parse().unwrap();
        assert_eq!(n.next_instance(&a, "lynx"), 1);
        assert_eq!(n.next_instance(&a, "lynx"), 2);
        assert_eq!(n.next_instance(&b, "lynx"), 1);
        assert_eq!(n.next_instance(&a, "otter"), 1);
    }

    #[test]
    fn exhausted_pool_falls_back_to_numbered_words() {
        let mut n = CrewNamer::new("jade-reef");
        for i in 0..crate::ROLES.len() {
            n.allocate_role(&format!("def{i}"));
        }
        let extra = n.allocate_role("one-more");
        assert!(extra.ends_with('2'), "{extra}");
    }

    #[test]
    fn shared_namer_persists_and_reloads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("run").join("codenames.json");
        let s = SharedNamer::open_or_init(path.clone(), || CrewNamer::new("jade-reef"));
        let parent: Codename = "jade-reef/heron".parse().unwrap();
        assert_eq!(s.with(|n| n.next_instance(&parent, "lynx")), 1);
        let again = SharedNamer::open_or_init(path, || panic!("must load, not init"));
        assert_eq!(again.with(|n| n.next_instance(&parent, "lynx")), 2);
        assert_eq!(again.crew(), "jade-reef");
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p rupu-codename namer`
Expected: compile error — `CrewNamer` not found.

- [ ] **Step 3: Implement**

```rust
//! Per-crew role allocation + instance counters. Deterministic given the
//! same sequence of calls, which is what lets resume recompute static slots.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::hash::fnv1a64;
use crate::{Codename, ROLES};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrewNamer {
    pub crew: String,
    /// agent def → words allocated to it, in order; `[0]` is canonical.
    #[serde(default)]
    roles: BTreeMap<String, Vec<String>>,
    /// `"<parent codename>>"<role>` → last issued instance number.
    #[serde(default)]
    counters: BTreeMap<String, u32>,
}

impl CrewNamer {
    pub fn new(crew: impl Into<String>) -> Self {
        Self { crew: crew.into(), ..Self::default() }
    }

    fn taken(&self, word: &str) -> bool {
        self.roles.values().flatten().any(|w| w == word)
    }

    fn probe(&self, agent_def: &str) -> String {
        let start = (fnv1a64(agent_def) % ROLES.len() as u64) as usize;
        for i in 0..ROLES.len() {
            let w = ROLES[(start + i) % ROLES.len()];
            if !self.taken(w) {
                return w.to_string();
            }
        }
        // More distinct slots than words in one crew: number the base word.
        let base = ROLES[start];
        (2..)
            .map(|k| format!("{base}{k}"))
            .find(|w| !self.taken(w))
            .unwrap_or_else(|| base.to_string())
    }

    /// A new word for a new static slot of `agent_def`.
    pub fn allocate_role(&mut self, agent_def: &str) -> String {
        let w = self.probe(agent_def);
        self.roles.entry(agent_def.to_string()).or_default().push(w.clone());
        w
    }

    /// The def's canonical word, allocating on first use (dynamic dispatch).
    pub fn canonical_role(&mut self, agent_def: &str) -> String {
        match self.roles.get(agent_def).and_then(|v| v.first()) {
            Some(w) => w.clone(),
            None => self.allocate_role(agent_def),
        }
    }

    /// Record a word decided elsewhere (a placed unit's coordinator) as canonical.
    pub fn seed_role(&mut self, agent_def: &str, word: &str) {
        let v = self.roles.entry(agent_def.to_string()).or_default();
        if !v.iter().any(|w| w == word) {
            v.insert(0, word.to_string());
        }
    }

    pub fn next_instance(&mut self, parent: &Codename, role: &str) -> u32 {
        let c = self.counters.entry(format!("{parent}>{role}")).or_insert(0);
        *c += 1;
        *c
    }
}

/// A `CrewNamer` shared by the orchestrator and the sub-agent dispatcher,
/// optionally persisted as JSON after every change.
#[derive(Debug, Clone)]
pub struct SharedNamer {
    inner: Arc<Mutex<CrewNamer>>,
    path: Option<PathBuf>,
}

impl SharedNamer {
    pub fn in_memory(namer: CrewNamer) -> Self {
        Self { inner: Arc::new(Mutex::new(namer)), path: None }
    }

    /// Load `path` if it holds a namer; otherwise build one with `init` and
    /// persist it. A corrupt file is logged and replaced by `init()`.
    pub fn open_or_init(path: PathBuf, init: impl FnOnce() -> CrewNamer) -> Self {
        let loaded = std::fs::read(&path)
            .ok()
            .and_then(|b| match serde_json::from_slice::<CrewNamer>(&b) {
                Ok(n) => Some(n),
                Err(e) => {
                    tracing::warn!(path = %path.display(), error = %e, "codenames.json unreadable; reinitialising");
                    None
                }
            });
        let fresh = loaded.is_none();
        let s = Self { inner: Arc::new(Mutex::new(loaded.unwrap_or_else(init))), path: Some(path) };
        if fresh {
            s.persist(&s.snapshot());
        }
        s
    }

    fn snapshot(&self) -> CrewNamer {
        match self.inner.lock() {
            Ok(g) => g.clone(),
            Err(p) => p.into_inner().clone(),
        }
    }

    fn persist(&self, state: &CrewNamer) {
        let Some(path) = &self.path else { return };
        let write = || -> std::io::Result<()> {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let tmp = path.with_extension("json.tmp");
            std::fs::write(&tmp, serde_json::to_vec_pretty(state)?)?;
            std::fs::rename(&tmp, path)
        };
        if let Err(e) = write() {
            // Names are already stored on the records that carry them; the
            // file only protects resume/counters, so a failed write degrades
            // rather than fails the run.
            tracing::warn!(path = %path.display(), error = %e, "failed to persist codenames.json");
        }
    }

    /// Run `f` against the namer; persist if it changed state.
    pub fn with<R>(&self, f: impl FnOnce(&mut CrewNamer) -> R) -> R {
        let mut guard = match self.inner.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        let before = guard.clone();
        let out = f(&mut guard);
        if *guard != before {
            let state = guard.clone();
            drop(guard);
            self.persist(&state);
        }
        out
    }

    pub fn crew(&self) -> String {
        self.snapshot().crew
    }
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p rupu-codename && cargo clippy -p rupu-codename --all-targets -- -D warnings`
Expected: PASS, clean.

- [ ] **Step 5: Commit**

```bash
rustfmt --edition 2021 crates/rupu-codename/src/namer.rs crates/rupu-codename/src/lib.rs
git add crates/rupu-codename
git commit -m "feat(codename): CrewNamer role allocation + persisted SharedNamer"
```

---

### Task 3: Agent runtime carries the codename (RunStart, system prompt, ToolContext, coverage attribution)

**Files:**
- Modify: `crates/rupu-transcript/src/event.rs:49` (`Event::RunStart`), plus any exhaustive constructors the compiler flags (`event.rs:613` test, `aggregate.rs:238` test)
- Modify: `crates/rupu-tools/src/tool.rs:37` (`ToolContext` + `impl Default` at :102)
- Modify: `crates/rupu-agent/src/runner.rs:595` (`AgentRunOpts`), `:909-942` (`run_agent`)
- Modify: `crates/rupu-agent/Cargo.toml` (add `rupu-codename = { workspace = true }`)
- Modify: `crates/rupu-coverage/src/ledger/events.rs:5` (`Attribution`), `crates/rupu-agent/src/coverage_tools.rs:23`, `crates/rupu-tools/src/coverage_emit.rs:13`
- Modify: every `AgentRunOpts { … }`, `ToolContext { … }`, `Attribution { … }` literal the compiler reports (non-test: `rupu-cli/src/cmd/{dispatch.rs:292,307, session.rs:7477,~7537, run.rs:805,~861}`, `rupu-cli/src/step_factory.rs:~372,412`; tests across crates)
- Test: `crates/rupu-agent/src/runner.rs` tests module

**Interfaces:**
- Produces:
  - `AgentRunOpts.codename: Option<String>` (full codename string)
  - `ToolContext.codename: Option<String>` (`#[serde(skip)]`)
  - `rupu_transcript::Event::RunStart { …, codename: Option<String> }` (`#[serde(skip_serializing_if = "Option::is_none", default)]`)
  - `rupu_coverage::Attribution { run_id, model, surface, codename: Option<String> }` (`#[serde(default, skip_serializing_if = "Option::is_none")]`)
  - `pub fn call_sign_line(codename: &str) -> Option<String>` in `rupu-agent/src/runner.rs` (pub(crate) is fine)

- [ ] **Step 1: Write the failing test** in `rupu-agent/src/runner.rs` tests (reuse the module's existing mock-provider helper — find the test that asserts `RunStart` / `system_prompt` is written and copy its setup; add `codename: Some("jade-reef/heron#3".into())` to the opts):

```rust
#[tokio::test]
async fn run_start_records_codename_and_prompt_carries_call_sign() {
    let tmp = tempfile::tempdir().unwrap();
    let transcript = tmp.path().join("t.jsonl");
    let mut opts = /* existing helper that builds AgentRunOpts with a MockProvider
                      scripted to a single EndTurn text reply, writing to `transcript` */;
    opts.codename = Some("jade-reef/heron#3".into());
    let result = run_agent(opts).await.unwrap();
    assert!(result.terminal_error().is_none());

    let first = rupu_transcript::JsonlReader::iter(&transcript).unwrap().next().unwrap().unwrap();
    match first {
        rupu_transcript::Event::RunStart { codename, system_prompt, .. } => {
            assert_eq!(codename.as_deref(), Some("jade-reef/heron#3"));
            let sp = system_prompt.unwrap();
            assert!(sp.contains("Your call sign in this run is `heron#3` (crew `jade-reef`)"), "{sp}");
        }
        other => panic!("first event must be RunStart, got {other:?}"),
    }
}

#[test]
fn call_sign_line_ignores_garbage() {
    assert!(call_sign_line("not a codename").is_none());
    assert!(call_sign_line("jade-reef").is_none()); // crew-only: no member to name
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p rupu-agent run_start_records_codename`
Expected: compile error — no field `codename`.

- [ ] **Step 3: Implement**

`event.rs` `RunStart`, after `system_prompt`:
```rust
        /// Human codename of this agent instance (`crew/role#n>…`).
        /// `None` on transcripts written before codenames existed.
        #[serde(skip_serializing_if = "Option::is_none", default)]
        codename: Option<String>,
```
`tool.rs` `ToolContext`: `#[serde(skip)] pub codename: Option<String>,` and `codename: None` in `impl Default`.
`Attribution`:
```rust
    /// Codename of the declaring agent instance. `None` for legacy records
    /// and for the per-workflow MCP `findings.record` path (crew only there).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codename: Option<String>,
```
`coverage_tools.rs:23` and `coverage_emit.rs:13`: add `codename: ctx.codename.clone(),`.
`AgentRunOpts`: add
```rust
    /// Human codename for this instance. Written to `RunStart`, announced in
    /// the system prompt, and copied onto `tool_context` for attribution.
    pub codename: Option<String>,
```
In `run_agent`, next to the coverage-section append at ~909 (before `RunStart` is written):
```rust
    if let Some(line) = opts.codename.as_deref().and_then(call_sign_line) {
        opts.agent_system_prompt.push_str(&line);
    }
```
Add `codename: opts.codename.clone(),` to the `Event::RunStart` literal (~913) and, at ~936-942 where `tool_context` fields are set, `opts.tool_context.codename = opts.codename.clone();`.
```rust
/// `\n\nYour call sign in this run is `heron#3` (crew `jade-reef`). …` —
/// `None` for anything that isn't a member codename.
pub(crate) fn call_sign_line(codename: &str) -> Option<String> {
    let c: rupu_codename::Codename = codename.parse().ok()?;
    if c.segments.is_empty() {
        return None;
    }
    Some(format!(
        "\n\nYour call sign in this run is `{}` (crew `{}`). Sign any comments, issues or PR notes you post with it.",
        c.leaf(),
        c.crew
    ))
}
```
Then add `codename: None` to every other literal the compiler reports (`cargo build --workspace --tests 2>&1 | rg -B2 'missing field .codename.'`). Sites that will get real values in later tasks still get `None` now.

- [ ] **Step 4: Run tests**

Run: `cargo test -p rupu-agent -p rupu-transcript -p rupu-tools -p rupu-coverage && cargo build --workspace --tests`
Expected: PASS; whole workspace builds. Also confirm an old transcript still parses: `cargo test -p rupu-transcript` (existing fixtures lack `codename`).

- [ ] **Step 5: Commit**

```bash
rustfmt --edition 2021 <each changed .rs file>
git add -A crates
git commit -m "feat(agent): codename on RunStart, call-sign prompt line, ToolContext + coverage attribution"
```

---

### Task 4: Orchestrator record + event fields (incl. new `AgentStarted`)

**Files:**
- Modify: `crates/rupu-orchestrator/Cargo.toml` (add `rupu-codename = { workspace = true }`)
- Modify: `crates/rupu-orchestrator/src/runs.rs` — `RunRecord` (:104), `StepResultRecord` (:565), `FindingRecord` (:640), `ItemResultRecord` (:648), `UnitCheckpoint` (:690), and the `From` impls at :707/:723/:739/:769/:786
- Modify: `crates/rupu-orchestrator/src/runner.rs` — `Finding` (:542), `ItemResult` (:580), `UnitDispatch` (:121)
- Modify: `crates/rupu-orchestrator/src/executor/event.rs` — `StepStarted`, `UnitStarted`, `DispatchStarted`, new `AgentStarted`, and `impl Event::run_id`
- Modify: every literal / exhaustive match the compiler reports (orchestrator, rupu-cli, rupu-cp incl. tests; `rupu-cp/tests/macos_fixtures.rs` may need its fixture regenerated — see Step 4)
- Test: `crates/rupu-orchestrator/src/runs.rs` tests, `executor/event.rs` tests

**Interfaces:**
- Produces (all `Option<String>`, `#[serde(default, skip_serializing_if = "Option::is_none")]`):
  - `RunRecord.codename` (crew) ; `StepResultRecord.codename` (singleton member; `None` for fan-out/panel/parallel steps whose instances live on `items`) ; `ItemResult.codename` / `ItemResultRecord.codename` ; `UnitCheckpoint.codename` ; `Finding.codename` / `FindingRecord.codename` ; `UnitDispatch.codename`
  - `Event::StepStarted { …, codename }`, `Event::UnitStarted { …, codename }`, `Event::DispatchStarted { …, codename, provider, model }`
  - New variant:
    ```rust
    /// An agent instance is about to run: the first moment its provider and
    /// model are known. One per agent instance (linear step, fan-out unit,
    /// parallel sub-step, panelist, fixer, on_reject cleanup). Placed units
    /// run remotely, so `provider`/`model` are `None` for them.
    AgentStarted {
        run_id: String,
        step_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        unit_index: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        codename: Option<String>,
        agent: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        agent_run_id: String,
        transcript_path: PathBuf,
    },
    ```

- [ ] **Step 1: Write the failing tests**

In `runs.rs` tests:
```rust
#[test]
fn legacy_records_round_trip_without_codename_keys() {
    let legacy = r#"{"step_id":"s","index":0,"item":1,"run_id":"run_X","transcript_path":"/t","output":"","success":true,"finished_at":"2026-09-29T00:00:00Z"}"#;
    let cp: UnitCheckpoint = serde_json::from_str(legacy).unwrap();
    assert!(cp.codename.is_none());
    assert_eq!(serde_json::to_string(&cp).unwrap(), legacy);
}

#[test]
fn codename_survives_step_result_conversion() {
    let item = crate::runner::ItemResult {
        index: 0, item: serde_json::json!(1), sub_id: "0".into(), rendered_prompt: String::new(),
        run_id: "run_U".into(), transcript_path: "/t".into(), output: String::new(), success: true,
        is_fixer: false, codename: Some("jade-reef/heron#1".into()),
    };
    let rec = ItemResultRecord::from(&item);
    assert_eq!(rec.codename.as_deref(), Some("jade-reef/heron#1"));
    assert_eq!(crate::runner::ItemResult::from(&rec).codename, item.codename);
}
```
In `executor/event.rs` tests:
```rust
#[test]
fn agent_started_serde_and_run_id() {
    let ev = Event::AgentStarted {
        run_id: "run_W".into(), step_id: "review".into(), unit_index: Some(3),
        codename: Some("jade-reef/heron#4".into()), agent: "security-reviewer".into(),
        provider: Some("anthropic".into()), model: Some("claude-opus-5-5".into()),
        agent_run_id: "run_U".into(), transcript_path: "/t.jsonl".into(),
    };
    let v = serde_json::to_value(&ev).unwrap();
    assert_eq!(v["type"], "agent_started");
    assert_eq!(v["codename"], "jade-reef/heron#4");
    assert_eq!(ev.run_id(), "run_W");
    let legacy = r#"{"type":"unit_started","run_id":"r","step_id":"s","index":0,"unit_key":"k","agent":null,"transcript_path":"/t"}"#;
    assert!(serde_json::from_str::<Event>(legacy).is_ok());
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p rupu-orchestrator --lib legacy_records_round_trip codename_survives agent_started`
Expected: compile errors (no field `codename`, no variant `AgentStarted`).

- [ ] **Step 3: Implement**

Add each field with a one-line doc comment and the serde attrs above. Thread `codename` through the `From<&ItemResult> for ItemResultRecord`, `From<&ItemResultRecord> for ItemResult`, the `Finding`↔`FindingRecord` maps (runs.rs:723 / ~769), and `StepResult`→`StepResultRecord` if `StepResult` gains it (add `pub codename: Option<String>` to `StepResult` too, so linear steps can set it). Add the `AgentStarted` arm to `impl Event { pub fn run_id }`. Fix every literal/match the compiler reports with `codename: None` (and `provider: None, model: None` for `DispatchStarted`); in `match` expressions over `Event` that are exhaustive without `_`, add an `Event::AgentStarted { .. }` arm doing what the neighbouring informational arms (`StepWorking`) do — in `rupu-cp` (`api/graph.rs`, `api/events.rs`, `sse.rs`) that is typically "ignore"; do NOT build CP display here.

- [ ] **Step 4: Run tests**

Run: `cargo test -p rupu-orchestrator && cargo build --workspace --tests && cargo test -p rupu-cp`
Expected: PASS. If `rupu-cp`'s macOS fixture drift test fails because a serialized shape gained an optional key, run `make macos-fixtures` and commit the regenerated `apps/rupu-macos/Fixtures/*.json` (fixture files only — no Swift changes).

- [ ] **Step 5: Commit**

```bash
git add -A crates apps/rupu-macos/Fixtures
git commit -m "feat(orchestrator): codename fields on records/events + AgentStarted event"
```

---

### Task 5: `RunNaming` — static-slot walk

**Files:**
- Create: `crates/rupu-orchestrator/src/codenames.rs`
- Modify: `crates/rupu-orchestrator/src/lib.rs` (`pub mod codenames;`)

**Interfaces:**
- Consumes: `CrewNamer`, `SharedNamer`, `Codename`, `crew_for` (Tasks 1–2); `Workflow`, `Step`, `Panel`, `PanelGate.fix_with`, `Approval.on_reject`, `SubStep` (workflow.rs)
- Produces:
  ```rust
  pub struct RunNaming { /* namer: SharedNamer, crew: Codename, slots: BTreeMap<SlotKey, String> */ }
  #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
  pub enum SlotKey { Step(String), Sub { step: String, sub: String }, Panelist { step: String, agent: String }, Fixer(String) }
  impl RunNaming {
      pub fn open(wf: &Workflow, run_id: &str, run_dir: Option<&Path>) -> Self;
      pub fn namer(&self) -> SharedNamer;
      pub fn crew(&self) -> &Codename;
      pub fn step(&self, step_id: &str, agent: &str) -> Codename;                    // crew/role
      pub fn unit(&self, step_id: &str, agent: &str, index: usize) -> Codename;      // crew/role#(index+1)
      pub fn sub(&self, step_id: &str, sub_id: &str, agent: &str) -> Codename;       // parallel / on_reject
      pub fn panelist(&self, step_id: &str, agent: &str, occurrence: Option<u32>) -> Codename;
      pub fn fixer(&self, step_id: &str, agent: &str) -> Codename;
  }
  ```
  Each constructor falls back to `namer.with(|n| n.canonical_role(agent))` when the slot is missing (e.g. a step added by a future feature), so a codename is always produced.

- [ ] **Step 1: Write the failing tests** (bottom of `codenames.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const WF: &str = r#"
name: t
steps:
  - id: alpha
    agent: ag
    actions: []
    prompt: "a"
  - id: beta
    agent: ag
    actions: []
    prompt: "b"
  - id: fan
    agent: triage
    actions: []
    for_each: "{{ inputs.items }}"
    prompt: "c"
"#;
    const RUN: &str = "run_01J9ZQ3K4M5N6P7Q8R9S0T1V2W"; // crew jade-reef

    #[test]
    fn second_slot_of_same_def_gets_a_distinct_role() {
        let wf = Workflow::parse(WF).unwrap();
        let n = RunNaming::open(&wf, RUN, None);
        assert_eq!(n.crew().to_string(), "jade-reef");
        assert_eq!(n.step("alpha", "ag").to_string(), "jade-reef/hedgehog");
        assert_eq!(n.step("beta", "ag").to_string(), "jade-reef/heron");
        assert_eq!(n.unit("fan", "triage", 411).to_string(), "jade-reef/numbat#412");
    }

    #[test]
    fn walk_is_deterministic_and_reloads_dynamic_state() {
        let wf = Workflow::parse(WF).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let a = RunNaming::open(&wf, RUN, Some(dir.path()));
        let parent = a.step("alpha", "ag");
        a.namer().with(|n| n.next_instance(&parent, "lynx"));
        let b = RunNaming::open(&wf, RUN, Some(dir.path()));
        assert_eq!(b.step("beta", "ag"), a.step("beta", "ag"));
        assert_eq!(b.namer().with(|n| n.next_instance(&parent, "lynx")), 2);
    }

    #[test]
    fn unknown_slot_falls_back_to_canonical_role() {
        let wf = Workflow::parse(WF).unwrap();
        let n = RunNaming::open(&wf, RUN, None);
        assert_eq!(n.step("ghost", "ag").to_string(), "jade-reef/hedgehog");
    }
}
```
(If `Workflow::parse` rejects `for_each` without an input declaration, copy a valid `for_each` fixture from `tests/linear_runner.rs` or `workflow.rs` tests.)

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p rupu-orchestrator --lib codenames`
Expected: compile error — module missing.

- [ ] **Step 3: Implement**

```rust
//! Per-run codename assignment (spec §4.3). The static walk is a pure
//! function of the workflow, so resume recomputes the same words; dynamic
//! state (sub-agent roles, instance counters) persists to
//! `<run_dir>/codenames.json` via `SharedNamer`.

use std::collections::BTreeMap;
use std::path::Path;

use rupu_codename::{crew_for, Codename, CrewNamer, SharedNamer};

use crate::workflow::Workflow;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum SlotKey {
    Step(String),
    Sub { step: String, sub: String },
    Panelist { step: String, agent: String },
    Fixer(String),
}

#[derive(Debug, Clone)]
pub struct RunNaming {
    namer: SharedNamer,
    crew: Codename,
    slots: BTreeMap<SlotKey, String>,
}

fn walk(wf: &Workflow, namer: &mut CrewNamer) -> BTreeMap<SlotKey, String> {
    let mut slots = BTreeMap::new();
    for step in &wf.steps {
        if let Some(agent) = &step.agent {
            slots.insert(SlotKey::Step(step.id.clone()), namer.allocate_role(agent));
        }
        if let Some(subs) = &step.parallel {
            for s in subs {
                slots.insert(
                    SlotKey::Sub { step: step.id.clone(), sub: s.id.clone() },
                    namer.allocate_role(&s.agent),
                );
            }
        }
        if let Some(panel) = &step.panel {
            let mut seen = std::collections::BTreeSet::new();
            for p in &panel.panelists {
                if seen.insert(p.clone()) {
                    slots.insert(
                        SlotKey::Panelist { step: step.id.clone(), agent: p.clone() },
                        namer.allocate_role(p),
                    );
                }
            }
            if let Some(gate) = &panel.gate {
                slots.insert(SlotKey::Fixer(step.id.clone()), namer.allocate_role(&gate.fix_with));
            }
        }
        if let Some(ap) = &step.approval {
            for s in &ap.on_reject {
                if let Some(agent) = &s.agent {
                    slots.insert(
                        SlotKey::Sub { step: step.id.clone(), sub: s.id.clone() },
                        namer.allocate_role(agent),
                    );
                }
            }
        }
    }
    slots
}

impl RunNaming {
    pub fn open(wf: &Workflow, run_id: &str, run_dir: Option<&Path>) -> Self {
        let crew = crew_for(run_id);
        let mut fresh = CrewNamer::new(crew.clone());
        let slots = walk(wf, &mut fresh);
        let namer = match run_dir {
            Some(dir) => SharedNamer::open_or_init(dir.join("codenames.json"), || fresh),
            None => SharedNamer::in_memory(fresh),
        };
        Self { namer, crew: Codename::crew_only(crew), slots }
    }

    pub fn namer(&self) -> SharedNamer {
        self.namer.clone()
    }

    pub fn crew(&self) -> &Codename {
        &self.crew
    }

    fn role(&self, key: SlotKey, agent: &str) -> String {
        match self.slots.get(&key) {
            Some(w) => w.clone(),
            None => self.namer.with(|n| n.canonical_role(agent)),
        }
    }

    pub fn step(&self, step_id: &str, agent: &str) -> Codename {
        self.crew.child(&self.role(SlotKey::Step(step_id.into()), agent), None)
    }

    pub fn unit(&self, step_id: &str, agent: &str, index: usize) -> Codename {
        let n = u32::try_from(index + 1).unwrap_or(u32::MAX);
        self.crew.child(&self.role(SlotKey::Step(step_id.into()), agent), Some(n))
    }

    pub fn sub(&self, step_id: &str, sub_id: &str, agent: &str) -> Codename {
        let key = SlotKey::Sub { step: step_id.into(), sub: sub_id.into() };
        self.crew.child(&self.role(key, agent), None)
    }

    /// `occurrence` is `Some(k)` only when `agent` appears more than once in
    /// the panel (spec §3: a singleton panelist carries no number).
    pub fn panelist(&self, step_id: &str, agent: &str, occurrence: Option<u32>) -> Codename {
        let key = SlotKey::Panelist { step: step_id.into(), agent: agent.into() };
        self.crew.child(&self.role(key, agent), occurrence)
    }

    pub fn fixer(&self, step_id: &str, agent: &str) -> Codename {
        self.crew.child(&self.role(SlotKey::Fixer(step_id.into()), agent), None)
    }
}
```
Verify the field names against `workflow.rs` (`Step.approval: Option<Approval>`, `Approval.on_reject: Vec<Step>`, `Panel.gate: Option<PanelGate>`, `PanelGate.fix_with: String`) and adjust if any are named differently.

- [ ] **Step 4: Run tests**

Run: `cargo test -p rupu-orchestrator --lib codenames && cargo clippy -p rupu-orchestrator --all-targets -- -D warnings`
Expected: PASS, clean.

- [ ] **Step 5: Commit**

```bash
rustfmt --edition 2021 crates/rupu-orchestrator/src/codenames.rs crates/rupu-orchestrator/src/lib.rs
git add -A crates/rupu-orchestrator
git commit -m "feat(orchestrator): RunNaming static-slot codename walk"
```

---

### Task 6: Runner mints codenames at every agent site + emits `AgentStarted`

**Files:**
- Modify: `crates/rupu-orchestrator/src/runner.rs` — `OrchestratorRunOpts` (:377), `run_workflow` (:803, RunRecord at :871), `run_reject_cleanup` (:5587, :5762), `dispatch_placed_step` (UnitDispatch :6018), `run_linear_step` (:6099/:6121), `run_fanout_step` (:6488; :6607, :6794, :6844/:6858 retry, UnitCheckpoint :7056, ItemResult ~:7101/~:7127, UnitStarted emits), `run_parallel_step` (:7215/:7239, ItemResult ~:7333), `run_panel_step` / `dispatch_fixer` (:7973/:7982, ItemResult :7814/:7848), `run_panel_iteration` (:8051/:8089, Finding ~:8212, ItemResult ~:8229), `dispatch_one` (:7449), every `StepStarted` emit
- Modify: `crates/rupu-orchestrator/src/executor/in_process.rs:249` (synthesized `RunRecord` stub: `codename: Some(rupu_codename::crew_for(&run_id))`)
- Modify: every `OrchestratorRunOpts { … }` literal (add `naming: None`) — tests and `rupu-cli`
- Test: `crates/rupu-orchestrator/tests/runner_events.rs`

**Interfaces:**
- Consumes: `RunNaming` (Task 5), fields from Task 4, `AgentRunOpts.codename` (Task 3)
- Produces:
  - `OrchestratorRunOpts.naming: Option<std::sync::Arc<crate::codenames::RunNaming>>` — callers that also build a sub-agent dispatcher pass one in (Task 7); otherwise `run_workflow` / `run_reject_cleanup` build their own via `RunNaming::open(&opts.workflow, &run_id, run_store.map(|s| s.root.join(&run_id)))`.
  - `dispatch_one(…, announce: AgentAnnounce)` where
    ```rust
    struct AgentAnnounce<'a> {
        sink: Option<&'a Arc<dyn crate::executor::EventSink>>,
        workflow_run_id: &'a str,
        unit_index: Option<usize>,
        codename: Option<Codename>,
    }
    ```

- [ ] **Step 1: Write the failing test** — append to `tests/runner_events.rs` (reuses `FakeFactory`, `CollectSink`, `WF_TWO_STEPS` in that file; alpha and beta both run agent `ag`):

```rust
#[tokio::test]
async fn every_agent_instance_is_announced_with_codename_provider_and_model() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let sink: Arc<CollectSink> = Arc::new(CollectSink::default());
    let wf = Workflow::parse(WF_TWO_STEPS).unwrap();
    let opts = OrchestratorRunOpts {
        run_step: Default::default(),
        workflow: wf,
        inputs: std::collections::BTreeMap::new(),
        workspace_id: "ws_names".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().to_path_buf(),
        factory: Arc::new(FakeFactory),
        event: None,
        run_store: None,
        workflow_yaml: None,
        resume_from: None,
        issue: None,
        issue_ref: None,
        run_id_override: Some("run_01J9ZQ3K4M5N6P7Q8R9S0T1V2W".into()),
        strict_templates: false,
        event_sink: Some(sink.clone() as Arc<dyn EventSink>),
        unit_dispatcher: None,
        action_dispatcher: None,
        pause: None,
        naming: None,
    };
    run_workflow(opts).await.unwrap();

    let events = sink.events.lock().unwrap();
    let started: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            Event::AgentStarted { step_id, codename, agent, provider, model, .. } => Some((
                step_id.clone(), codename.clone().unwrap(), agent.clone(),
                provider.clone().unwrap(), model.clone().unwrap(),
            )),
            _ => None,
        })
        .collect();
    assert_eq!(
        started,
        vec![
            ("alpha".into(), "jade-reef/hedgehog".into(), "ag".into(), "mock".into(), "mock-1".into()),
            ("beta".into(), "jade-reef/heron".into(), "ag".into(), "mock".into(), "mock-1".into()),
        ]
    );
    let step_names: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            Event::StepStarted { codename, .. } => codename.clone(),
            _ => None,
        })
        .collect();
    assert_eq!(step_names, vec!["jade-reef/hedgehog", "jade-reef/heron"]);
}
```
Also add `naming: None,` to the existing `OrchestratorRunOpts` literals in this file (they will not compile otherwise). Note `agent` in `AgentStarted` is the **workflow's** agent name (`ag`), not the factory's `agent_name` (`ag-ag`) — pass the name the workflow used.

Add a fan-out assertion to the existing fan-out test in `tests/linear_runner.rs` (find the test that runs a `for_each` step and inspects `StepResult.items`): assert `items[i].codename` ends with `#{i+1}` and all are distinct.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p rupu-orchestrator --test runner_events every_agent_instance`
Expected: compile error (`naming` field missing).

- [ ] **Step 3: Implement**

1. `OrchestratorRunOpts`: add
   ```rust
   /// Codename assignment for this run. `None` ⇒ `run_workflow` builds one;
   /// callers that also own a sub-agent dispatcher pass theirs so both share
   /// one `CrewNamer`.
   pub naming: Option<std::sync::Arc<crate::codenames::RunNaming>>,
   ```
   and a helper
   ```rust
   fn ensure_naming(opts: &mut OrchestratorRunOpts, run_id: &str) -> Arc<RunNaming> {
       if opts.naming.is_none() {
           let dir = opts.run_store.as_ref().map(|s| s.root.join(run_id));
           opts.naming = Some(Arc::new(RunNaming::open(&opts.workflow, run_id, dir.as_deref())));
       }
       opts.naming.clone().unwrap_or_else(|| unreachable!("set above"))
   }
   ```
   (If clippy's `disallowed_methods` forbids `unreachable!`, restructure as `let n = …; opts.naming = Some(n.clone()); n`.) Call it in `run_workflow` right after the run id is final (after resume/override handling, before the RunRecord at :871) and in `run_reject_cleanup`; set `codename: Some(naming.crew().to_string())` on the RunRecord literal.

2. `dispatch_one`: add `announce: AgentAnnounce<'_>` as the last parameter. After `build_opts_for_step` returns:
   ```rust
   agent_opts.codename = announce.codename.as_ref().map(ToString::to_string);
   if let Some(sink) = announce.sink {
       sink.emit(
           announce.workflow_run_id,
           &Event::AgentStarted {
               run_id: announce.workflow_run_id.to_string(),
               step_id: step_id.to_string(),
               unit_index: announce.unit_index,
               codename: agent_opts.codename.clone(),
               agent: agent_name.to_string(),
               provider: Some(agent_opts.provider_name.clone()),
               model: Some(agent_opts.model.clone()),
               agent_run_id: agent_opts.run_id.clone(),
               transcript_path: agent_opts.transcript_path.clone(),
           },
       );
   }
   ```

3. At each of the 7 `dispatch_one` call sites compute the codename from `opts.naming` and pass it; also store it on the record the site builds:
   - `run_linear_step` (:6121): `naming.step(&step.id, agent)`; set `StepStarted.codename`, `StepResult.codename`. Placed branch (`dispatch_placed_step`, UnitDispatch :6018): `codename: Some(c.to_string())` and emit `AgentStarted` directly with `provider: None, model: None, unit_index: None`.
   - `run_fanout_step` (:6607): per unit `naming.unit(&step.id, agent, idx)` → `UnitStarted.codename`, `ItemResult.codename`, `UnitCheckpoint.codename` (:7056), `UnitDispatch.codename` (:6794). Retry (:6844/:6858): `naming.unit(..).with_attempt(2)` for the retry's UnitDispatch/UnitStarted/ItemResult. Placed units emit `AgentStarted` with `provider/model: None`. Resumed units (6538–6560) keep the record's codename; if `None` (pre-codename checkpoint), use `naming.unit(..)`. The step-level `StepStarted.codename` for a fan-out step is `naming.step(&step.id, agent)` (the role without `#n`).
   - `run_fanout_run_step` (:6414/:6432): these are `run:` shell units, not agents — leave `codename: None`.
   - `run_parallel_step` (:7239): `naming.sub(&step.id, &sub.id, &sub.agent)` → `ItemResult.codename`, `AgentStarted`. Pass the parent's `workflow_run_id` into `run_parallel_step` if it lacks it (it currently has no such param — add `workflow_run_id: &str`).
   - `run_panel_iteration` (:8089): occurrence = `let count = panel.panelists.iter().filter(|p| *p == panelist).count(); (count > 1).then(|| panel.panelists[..=pos].iter().filter(|p| *p == panelist).count() as u32)` where `pos` is the panelist's position in `panel.panelists`; codename `naming.panelist(&step.id, panelist, occurrence)` → `UnitStarted.codename`, `ItemResult.codename`, each `Finding.codename` built at ~8212 (`Some(o.codename.clone())` — add `codename: Option<String>` to `PanelOutcome` :8281 to carry it).
   - `dispatch_fixer` (:7982): `naming.fixer(&step.id, fixer_agent)` → fixer `ItemResult.codename` (:7814/:7848).
   - `run_reject_cleanup` (:5762): `naming.sub(rejected_step_id, &sub.id, agent)`.
   Gate iterations and loop iterations reuse the same slot → same codename (spec §4.3); do not number them.

4. `in_process.rs:249`: `codename: Some(rupu_codename::crew_for(&run_id))`.

- [ ] **Step 4: Run tests**

Run: `cargo test -p rupu-orchestrator && cargo build --workspace --tests`
Expected: PASS (existing event-count assertions in `runner_events.rs` will now see extra `AgentStarted` events — update their expected sequences to include one `AgentStarted` right after each `StepWorking`/`UnitStarted` for a local agent step, and keep the comment explaining the sequence in sync).

- [ ] **Step 5: Commit**

```bash
rustfmt --edition 2021 crates/rupu-orchestrator/src/runner.rs crates/rupu-orchestrator/src/executor/in_process.rs crates/rupu-orchestrator/tests/runner_events.rs crates/rupu-orchestrator/tests/linear_runner.rs
git add -A crates
git commit -m "feat(orchestrator): mint codenames at every agent site; emit AgentStarted"
```

---

### Task 7: Sub-agent dispatch — `>role#n` children

**Files:**
- Modify: `crates/rupu-tools/src/tool.rs:125-160` (`AgentDispatcher::dispatch` signature, `DispatchOutcome.codename`)
- Modify: `crates/rupu-tools/src/dispatch_agent.rs:~155,167`, `crates/rupu-tools/src/dispatch_agents_parallel.rs:~207,245` (+ their test dispatchers)
- Modify: `crates/rupu-cli/Cargo.toml` (add `rupu-codename = { workspace = true }`)
- Modify: `crates/rupu-cli/src/cmd/dispatch.rs` (`CliAgentDispatcher` :26, `new` :91, `dispatch` :167-431)
- Modify: `crates/rupu-cli/src/cmd/workflow.rs` (~3215 `CliAgentDispatcher::new`; the `OrchestratorRunOpts` literal(s); `FindingsContext` at :3234/:4784), `crates/rupu-cli/src/cmd/resume.rs:317`
- Modify: `crates/rupu-mcp/src/tools/findings.rs:23,103` (`FindingsContext.codename`)
- Modify: `crates/rupu-orchestrator/tests/{dispatch_agent,dispatch_agents_parallel}.rs` test dispatchers
- Test: `crates/rupu-tools/src/dispatch_agent.rs` tests, `crates/rupu-cli/src/cmd/dispatch.rs` tests

**Interfaces:**
- Consumes: `SharedNamer`, `CrewNamer`, `Codename`; `ToolContext.codename` (Task 3); `RunNaming::namer()` (Task 5); `Event::DispatchStarted { codename, provider, model }` (Task 4)
- Produces:
  - `AgentDispatcher::dispatch(&self, agent_name: &str, prompt: String, parent_run_id: &str, parent_depth: u32, parent_codename: Option<&str>) -> Result<DispatchOutcome, DispatchError>`
  - `DispatchOutcome.codename: Option<String>` (`#[serde(default, skip_serializing_if = "Option::is_none")]`), surfaced in both tools' JSON result as `"codename"`
  - `CliAgentDispatcher::set_namer(&self, namer: SharedNamer)`
  - `pub(crate) fn child_codename(namer: &SharedNamer, parent: &str, agent: &str) -> Option<String>` in `dispatch.rs`
  - `FindingsContext.codename: Option<String>` → `Attribution.codename`

- [ ] **Step 1: Write the failing tests**

`rupu-tools/src/dispatch_agent.rs` tests — the existing test dispatcher returns `sub_TEST`; make it record the `parent_codename` it received and return `codename: Some("p>lynx#1".into())`, then:
```rust
#[tokio::test]
async fn passes_parent_codename_and_surfaces_child_codename() {
    let mut ctx = ctx_with(None, Some(vec!["reviewer".into()]), Some("run_X".into()), 0);
    ctx.codename = Some("jade-reef/heron#2".into());
    let (tool, seen) = /* existing recording-dispatcher setup */;
    let out = tool.invoke(serde_json::json!({"agent":"reviewer","prompt":"p"}), &ctx).await;
    let parsed: serde_json::Value = serde_json::from_str(&out.stdout).unwrap();
    assert_eq!(seen.lock().unwrap().as_deref(), Some("jade-reef/heron#2"));
    assert_eq!(parsed["codename"], "p>lynx#1");
}
```
`rupu-cli/src/cmd/dispatch.rs` tests:
```rust
#[test]
fn child_codename_numbers_per_parent_and_role() {
    let namer = rupu_codename::SharedNamer::in_memory(rupu_codename::CrewNamer::new("jade-reef"));
    let a = child_codename(&namer, "jade-reef/hedgehog", "security-reviewer").unwrap();
    let b = child_codename(&namer, "jade-reef/hedgehog", "security-reviewer").unwrap();
    let c = child_codename(&namer, "jade-reef/heron", "security-reviewer").unwrap();
    assert_eq!(a, "jade-reef/hedgehog>ferret#1");
    assert_eq!(b, "jade-reef/hedgehog>ferret#2");
    assert_eq!(c, "jade-reef/heron>ferret#1");
    assert!(child_codename(&namer, "garbage", "x").is_none());
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p rupu-tools passes_parent_codename; cargo test -p rupu-cli child_codename_numbers`
Expected: compile errors.

- [ ] **Step 3: Implement**

- Trait: add `parent_codename: Option<&str>` as the last param; update every impl (production + tests).
- Tools: pass `ctx.codename.as_deref()`; add `"codename": outcome.codename` next to `"sub_run_id"` in both JSON results.
- `CliAgentDispatcher`: add field `namer: std::sync::Mutex<Option<rupu_codename::SharedNamer>>` (init `Mutex::new(None)` in `new`), and
  ```rust
  pub fn set_namer(&self, namer: rupu_codename::SharedNamer) {
      if let Ok(mut g) = self.namer.lock() {
          *g = Some(namer);
      }
  }

  /// The run's namer, or — when none was installed (a dispatcher whose
  /// caller had no RunNaming) — an in-memory one for the parent's crew, so
  /// sub-agents are always named.
  fn namer_for(&self, parent: &rupu_codename::Codename) -> Option<rupu_codename::SharedNamer> {
      let mut g = self.namer.lock().ok()?;
      Some(g.get_or_insert_with(|| {
          rupu_codename::SharedNamer::in_memory(rupu_codename::CrewNamer::new(parent.crew.clone()))
      }).clone())
  }
  ```
  ```rust
  pub(crate) fn child_codename(namer: &rupu_codename::SharedNamer, parent: &str, agent: &str) -> Option<String> {
      let parent: rupu_codename::Codename = parent.parse().ok()?;
      Some(namer.with(|n| {
          let role = n.canonical_role(agent);
          let k = n.next_instance(&parent, &role);
          parent.child(&role, Some(k)).to_string()
      }))
  }
  ```
  In `dispatch`: compute `let codename = parent_codename.and_then(|p| { let parent = p.parse().ok()?; let namer = self.namer_for(&parent)?; child_codename(&namer, p, agent_name) });`. **Move** the `OrchEvent::DispatchStarted` emit (currently step 3, ~211) to just after the provider is built (step 5) so it can carry `provider: Some(provider_name.clone()), model: Some(model.clone())` plus `codename: codename.clone()`. Set `child_tool_ctx.codename = codename.clone()` (292), `AgentRunOpts.codename = codename.clone()` (307), and `DispatchOutcome.codename = codename` (431).
- `cmd/workflow.rs`: where the run id is known and `CliAgentDispatcher::new` is called (~3215) and the `OrchestratorRunOpts` literal is built, create `let naming = Arc::new(rupu_orchestrator::codenames::RunNaming::open(&workflow, &run_id, Some(&runs_dir.join(&run_id))));`, call `dispatcher.set_namer(naming.namer())`, and pass `naming: Some(naming.clone())`. Do the same in any other `OrchestratorRunOpts` construction in `rupu-cli` that has a dispatcher (grep `OrchestratorRunOpts {` in `crates/rupu-cli/src`); others pass `naming: None`.
- `FindingsContext`: add `pub codename: Option<String>` (crew only — doc comment says why: one context per workflow, not per step). Construct with `Some(rupu_codename::crew_for(&run_id))` at workflow.rs:3234/4784 and resume.rs:317; findings.rs:103 sets `Attribution.codename = ctx.codename.clone()`.

- [ ] **Step 4: Run tests**

Run: `cargo test -p rupu-tools -p rupu-orchestrator -p rupu-mcp && cargo test -p rupu-cli dispatch && cargo build --workspace --tests`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add -A crates
git commit -m "feat(dispatch): sub-agent codenames (>role#n), DispatchStarted provider/model"
```

---

### Task 8: Standalone runs, sessions, and placed/remote units

**Files:**
- Modify: `crates/rupu-cli/src/cmd/run.rs` (`run_inner` :492 — id mint :544-547, dispatcher ~:790, ToolContext :805, AgentRunOpts ~:861, RunRecord :1029)
- Modify: `crates/rupu-cli/src/cmd/session.rs` (`SessionRecord` ~:347, `start` literal ~:1555, `run_turn` ToolContext :7477 / AgentRunOpts ~:7537)
- Modify: `crates/rupu-cp/src/agent_launcher.rs:3` (`AgentLaunchRequest.codename`), `crates/rupu-cp/src/host/ssh.rs:1153` (`agent_argv`), `crates/rupu-cli/src/fleet_unit_dispatcher.rs:~299`, and every other `AgentLaunchRequest { … }` literal (`codename: None`)
- Test: `run.rs` tests, `session.rs` tests, `ssh.rs` tests

**Interfaces:**
- Consumes: `crew_for`, `role_word`, `derive_legacy`, `Codename`, `CrewNamer::seed_role`, `SharedNamer::open_or_init`, `UnitDispatch.codename`
- Produces:
  - `pub(crate) fn standalone_codename(run_id: &str, agent: &str, env_override: Option<String>) -> rupu_codename::Codename` in `run.rs`
  - `SessionRecord.codename: Option<String>` (`#[serde(default)]`)
  - `AgentLaunchRequest.codename: Option<String>`; SSH launches prefix `env RUPU_CODENAME=<codename>`
  - Env var contract: `RUPU_CODENAME` — `rupu run` uses it (if it parses and has a member segment) instead of deriving

- [ ] **Step 1: Write the failing tests**

`run.rs` tests:
```rust
#[test]
fn standalone_codename_derives_or_honours_env() {
    let derived = standalone_codename("run_01J9ZQ3K4M5N6P7Q8R9S0T1V2W", "triage", None);
    assert_eq!(derived.to_string(), "jade-reef/numbat");
    let placed = standalone_codename("run_01J9ZQ3K4M5N6P7Q8R9S0T1V2W", "triage", Some("cobalt-harbor/heron#412".into()));
    assert_eq!(placed.to_string(), "cobalt-harbor/heron#412");
    let junk = standalone_codename("run_01J9ZQ3K4M5N6P7Q8R9S0T1V2W", "triage", Some("junk".into()));
    assert_eq!(junk.to_string(), "jade-reef/numbat");
}
```
`ssh.rs` tests:
```rust
#[test]
fn agent_argv_prefixes_codename_env() {
    let req = AgentLaunchRequest {
        agent: "triage".into(), prompt: None, mode: None, target: None, working_dir: None,
        run_id: None, codename: Some("cobalt-harbor/heron#412".into()),
    };
    let argv = SshHostConnector::agent_argv(&req, "run_1");
    assert_eq!(&argv[..3], &["env".to_string(), "RUPU_CODENAME=cobalt-harbor/heron#412".into(), "rupu".into()]);
    assert!(build_remote_command(&argv).starts_with("'env' 'RUPU_CODENAME=cobalt-harbor/heron#412' 'rupu' 'run'"));
}
```
(Use the actual connector type name that owns `agent_argv` in `ssh.rs`.)
`session.rs` tests: find the test that builds/loads a `SessionRecord` from JSON; add
```rust
#[test]
fn legacy_session_record_loads_without_codename() {
    // take the smallest existing SessionRecord JSON fixture/test literal in this file
    let rec: SessionRecord = serde_json::from_str(LEGACY_SESSION_JSON).unwrap();
    assert!(rec.codename.is_none());
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p rupu-cli standalone_codename legacy_session_record; cargo test -p rupu-cp agent_argv_prefixes`
Expected: compile errors.

- [ ] **Step 3: Implement**

`run.rs`:
```rust
/// Codename for a standalone `rupu run`: a placed unit's coordinator passes
/// its minted name via `RUPU_CODENAME`; otherwise the run is its own crew.
pub(crate) fn standalone_codename(run_id: &str, agent: &str, env_override: Option<String>) -> rupu_codename::Codename {
    env_override
        .and_then(|s| s.parse::<rupu_codename::Codename>().ok())
        .filter(|c| !c.segments.is_empty())
        .unwrap_or_else(|| {
            rupu_codename::Codename::crew_only(rupu_codename::crew_for(run_id))
                .child(rupu_codename::role_word(agent), None)
        })
}
```
In `run_inner` after the id mint: `let codename = standalone_codename(&run_id, &spec.name, std::env::var("RUPU_CODENAME").ok());`. After `CliAgentDispatcher::new`: 
```rust
dispatcher.set_namer(rupu_codename::SharedNamer::open_or_init(
    runs_dir.join(&run_id).join("codenames.json"),
    || {
        let mut n = rupu_codename::CrewNamer::new(codename.crew.clone());
        if let Some(seg) = codename.segments.last() {
            n.seed_role(&spec.name, &seg.role);
        }
        n
    },
));
```
(use the runs dir variable `run.rs` already has for `RunStore`). Set `ToolContext.codename` and `AgentRunOpts.codename` to `Some(codename.to_string())`, and `RunRecord.codename: Some(codename.crew.clone())` at :1029.

`session.rs`: add to `SessionRecord`
```rust
    /// `crew/role` — one identity across every turn (spec §3). `None` for
    /// sessions created before codenames; readers use `derive_legacy`.
    #[serde(default)]
    codename: Option<String>,
```
In `start` (~1555): `codename: Some(rupu_codename::Codename::crew_only(rupu_codename::crew_for(&session_id)).child(rupu_codename::role_word(&agent_name), None).to_string()),` (use the local variable names in scope). In `run_turn`: `let codename = session.codename.clone().unwrap_or_else(|| rupu_codename::derive_legacy(&session.session_id, Some(&session.agent_name)));` → `ToolContext.codename` and `AgentRunOpts.codename`. (Session turns have `dispatcher: None`; no namer needed.)

`agent_launcher.rs`:
```rust
    /// Codename minted by a placed unit's coordinator. Forwarded to the
    /// remote `rupu run` as `RUPU_CODENAME` (an env var, so an older remote
    /// binary ignores it instead of rejecting an unknown flag). SSH only this
    /// arc; other connectors ignore it and the coordinator's records still
    /// carry the name.
    pub codename: Option<String>,
```
`ssh.rs` `agent_argv`: start with
```rust
let mut a: Vec<String> = Vec::new();
if let Some(c) = &req.codename {
    a.push("env".into());
    a.push(format!("RUPU_CODENAME={c}"));
}
a.extend(["rupu".to_string(), "run".into(), req.agent.clone()]);
```
`fleet_unit_dispatcher.rs:~299`: `codename: unit.codename.clone(),` (clone before `unit.agent` is moved if needed). All other literals: `codename: None`.

- [ ] **Step 4: Run tests**

Run: `cargo test -p rupu-cli && cargo test -p rupu-cp && cargo build --workspace --tests`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add -A crates
git commit -m "feat(codename): standalone runs, sessions, placed units (RUPU_CODENAME)"
```

---

### Task 9: CLI display — headers, step/unit labels, live view, NAME columns

**Files:**
- Create: `crates/rupu-cli/src/output/codename.rs` (+ `pub mod codename;` in `output/mod.rs`)
- Modify: `crates/rupu-cli/src/output/printer.rs` (`workflow_header` :287, `agent_header` :309, `session_header` :324), callers `workflow_printer.rs:340`, `cmd/watch.rs:156`, `cmd/run.rs:673`, `cmd/session.rs` (session_header callers)
- Modify: `crates/rupu-cli/src/output/live_run.rs` (`StepState`, `UnitState` :64, `ActiveFocus`, `apply` :382 — `StepStarted` :388, `UnitStarted` :444, `DispatchStarted` :541, new `AgentStarted` arm; `render_graph` :982/:1057; `render_focus` :1161)
- Modify: `crates/rupu-cli/src/cmd/workflow.rs` (`WorkflowRunsRow` :587, row build ~:2403, CSV headers ~:900, `render_workflow_runs_table` :932)
- Modify: `crates/rupu-cli/src/cmd/session.rs` (`SessionListRow` :464, `SessionListCsvRow`, row build :1188, `render_session_list_table` :633)
- Modify: `crates/rupu-cli/src/cmd/transcript.rs` (`TranscriptListRow` :205, row build :1869, `build_transcript_list_table` ~:352)
- Test: `output/codename.rs` tests; existing snapshot tests for the three tables and `live_run.rs` (update snapshots with `cargo insta review`/`INSTA_UPDATE=always` only after eyeballing the diff)

**Interfaces:**
- Consumes: `Codename`, `crew_tint`, `role_badge`, `derive_legacy`; `RunRecord.codename`, `SessionRecord.codename`, `RunStart.codename`, `Event::{StepStarted,UnitStarted,DispatchStarted,AgentStarted}` fields
- Produces (in `output/codename.rs`):
  - `pub fn display_codename(stored: Option<&str>, id: &str, agent: Option<&str>) -> String` — stored, else `derive_legacy(id, agent)`
  - `pub fn write_crew(buf: &mut String, crew: &str)` — tint dot `●` in the crew color + bold name (plain under no-color, via the same `palette::write_colored` helpers `printer.rs` uses)
  - `pub fn write_member(buf: &mut String, codename: &str)` — role badge glyph in its hue + leaf
  - `pub fn member_label(codename: Option<&str>, agent: &str, provider: Option<&str>, model: Option<&str>) -> String` — plain text `heron#412 · security-reviewer · anthropic/claude-opus-5-5` (parts omitted when `None`)

- [ ] **Step 1: Write the failing tests** (`output/codename.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_prefers_stored_then_derives() {
        assert_eq!(display_codename(Some("cobalt-harbor"), "run_X", None), "cobalt-harbor");
        assert_eq!(display_codename(None, "run_01J9ZQ3K4M5N6P7Q8R9S0T1V2W", None), "jade-reef");
    }

    #[test]
    fn member_label_joins_available_parts() {
        assert_eq!(
            member_label(Some("jade-reef/heron#4"), "security-reviewer", Some("anthropic"), Some("claude-opus-5-5")),
            "heron#4 · security-reviewer · anthropic/claude-opus-5-5"
        );
        assert_eq!(member_label(None, "triage", None, None), "triage");
    }

    #[test]
    fn write_member_contains_glyph_and_leaf() {
        let mut s = String::new();
        write_member(&mut s, "jade-reef/heron#4");
        let glyph = rupu_codename::role_badge("heron").shape.glyph();
        assert!(s.contains(glyph) && s.contains("heron#4"), "{s:?}");
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p rupu-cli output::codename`
Expected: compile error.

- [ ] **Step 3: Implement**

`output/codename.rs`:
```rust
//! Codename rendering for CLI output (spec §7).

use super::palette;
use rupu_codename::{crew_tint, derive_legacy, role_badge, Codename};

pub fn display_codename(stored: Option<&str>, id: &str, agent: Option<&str>) -> String {
    stored.map(str::to_string).unwrap_or_else(|| derive_legacy(id, agent))
}

fn rgb(t: rupu_codename::Tint) -> owo_colors::Rgb {
    let (r, g, b) = t.dark_rgb();
    owo_colors::Rgb(r, g, b)
}

pub fn write_crew(buf: &mut String, crew: &str) {
    if let Some(t) = crew_tint(crew) {
        let _ = palette::write_colored(buf, "●", rgb(t));
        buf.push(' ');
        let _ = palette::write_bold_colored(buf, crew, rgb(t));
    } else {
        buf.push_str(crew);
    }
}

pub fn write_member(buf: &mut String, codename: &str) {
    let Ok(c) = codename.parse::<Codename>() else {
        buf.push_str(codename);
        return;
    };
    match c.segments.last() {
        Some(seg) => {
            let badge = role_badge(&seg.role);
            let _ = palette::write_colored(buf, &badge.shape.glyph().to_string(), rgb(badge.tint));
            buf.push(' ');
            buf.push_str(&c.leaf());
        }
        None => write_crew(buf, &c.crew),
    }
}

pub fn member_label(codename: Option<&str>, agent: &str, provider: Option<&str>, model: Option<&str>) -> String {
    let mut parts = Vec::new();
    if let Some(c) = codename.and_then(|c| c.parse::<Codename>().ok()) {
        parts.push(c.leaf());
    }
    parts.push(agent.to_string());
    match (provider, model) {
        (Some(p), Some(m)) => parts.push(format!("{p}/{m}")),
        (None, Some(m)) => parts.push(m.to_string()),
        _ => {}
    }
    parts.join(" · ")
}
```
(`palette::write_colored` already honours no-color — confirm by reading `palette.rs:277`; if it doesn't, gate on the same flag `printer.rs` uses.)

Headers:
- `workflow_header(workflow_name, codename: Option<&str>, run_id, started_at)` → `▶ <workflow>  ● <crew>  <run_id>  HH:MM:SS` (write_crew when Some). Callers pass `record.codename` via `display_codename(record.codename.as_deref(), &record.id, None)`.
- `agent_header(agent_name, codename: Option<&str>, provider, model, run_id)` → `▶ <agent>  ◆ heron  (<provider> · <model>)  <run_id>` using `write_member`.
- `session_header(session_id, agent_name, codename: Option<&str>)` similarly.
- Where `workflow_printer.rs` calls `step_start(step_id, agent, …)` (2876, 3150, 3474), pass `agent` as `member_label(codename, agent, None, None)` when the record/event provides a codename (`StepResultRecord.codename`, `RunRecord.active_step_agent` + the step's codename from events if available; if no codename is at hand at a call site, leave it unchanged — don't fabricate).

Live view (`live_run.rs`):
- Add `codename: Option<String>` to `StepState` and `UnitState`; `provider: Option<String>, model: Option<String>` to `StepState`, `UnitState` and `ActiveFocus`.
- `apply`: `StepStarted` → `step.codename = codename.clone()`; `UnitStarted` → `unit.codename`; `DispatchStarted` → the sub-agent unit's codename/provider/model; new `WfEvent::AgentStarted { step_id, unit_index, codename, provider, model, .. }` → set on the unit at `unit_index` (or the step when `None`) and on `active` if it matches.
- `render_graph` (:1057): unit row label becomes `write_member(codename)` when set, else `unit.key` (unchanged); step row (:995) shows `member_label(step.codename, agent, step.provider, step.model)`.
- `render_focus` (:1161): header `format!("{unit} · {agent}")` → `member_label(active.codename, agent, active.provider, active.model)` prefixed by the unit key when it's a fan-out unit.
- Dashboard title (:833): append `  ● <crew>` via `write_crew` (`LiveRunState` gains `codename: Option<String>`, loaded from `run.json` at `run_live_view` start).

Tables — add a `NAME` column **first after the id** using `CellValue::Name(...)` (it's accepted back as an id in Task 10):
- `WorkflowRunsRow.codename: String` = `display_codename(r.codename.as_deref(), &r.id, None)`; column `"NAME"`; CSV header `name`.
- `SessionListRow.codename: String` = `display_codename(rec.codename.as_deref(), &rec.session_id, Some(&rec.agent_name))`; column `"NAME"`; CSV too.
- `TranscriptListRow.codename: Option<String>` from the transcript's `RunStart.codename` (the row builder already reads the `RunStart` for agent/status — take `codename` from the same match; legacy → `derive_legacy(run_id, Some(agent))`); column `"NAME"`.

- [ ] **Step 4: Run tests**

Run: `cargo test -p rupu-cli`
Expected: new tests PASS; table/live snapshot tests fail only on the added NAME column/labels. Review each snapshot diff, confirm only codename text was added, then accept (`INSTA_UPDATE=always cargo test -p rupu-cli` or `cargo insta accept`).

- [ ] **Step 5: Manual check**

```bash
cargo run -p rupu-cli -- workflow runs --limit 5
cargo run -p rupu-cli -- session list
cargo run -p rupu-cli -- transcript list --limit 5
```
Expected: a NAME column with `color-noun` crews for existing (legacy) runs; nothing else changed.

- [ ] **Step 6: Commit**

```bash
git add -A crates
git commit -m "feat(cli): show codenames in headers, live view, and run/session/transcript lists"
```

---

### Task 10: Codenames accepted wherever an id is

**Files:**
- Modify: `crates/rupu-cli/src/output/codename.rs` (resolution helpers)
- Modify: `crates/rupu-cli/src/cmd/workflow.rs:2507-2590` (`RunCandidate`, `resolve_run_fragment`)
- Modify: `crates/rupu-cli/src/cmd/session.rs:7972-8014` (`read_session`, `resolve_session_fragment`)
- Modify: `crates/rupu-cli/src/cmd/transcript.rs:2161` (`locate_transcript`)
- Modify: `crates/rupu-orchestrator/src/runs.rs` (`RunStore::find_instance`)
- Test: `output/codename.rs`, `runs.rs` tests

**Interfaces:**
- Produces:
  ```rust
  pub struct CrewCandidate { pub id: String, pub crew: String, pub started_at: chrono::DateTime<chrono::Utc> }
  pub enum CrewResolution { NotFound, Resolved { id: String, others: Vec<String> } }
  pub fn resolve_crew(candidates: &[CrewCandidate], crew: &str, now: DateTime<Utc>) -> CrewResolution
  pub fn ambiguity_note(crew: &str, chosen: &str, others: &[String]) -> String
  ```
  and `RunStore::find_instance(&self, run_id: &str, codename: &str) -> Result<Option<(String, PathBuf)>, RunStoreError>` returning `(agent_run_id, transcript_path)`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn crew_resolves_to_most_recent_and_lists_recent_others() {
    use chrono::{Duration, TimeZone, Utc};
    let now = Utc.with_ymd_and_hms(2026, 9, 29, 12, 0, 0).unwrap();
    let c = |id: &str, crew: &str, days: i64| CrewCandidate { id: id.into(), crew: crew.into(), started_at: now - Duration::days(days) };
    let cands = vec![c("run_old", "jade-reef", 90), c("run_mid", "jade-reef", 10), c("run_new", "jade-reef", 1), c("run_x", "olive-pine", 0)];
    match resolve_crew(&cands, "jade-reef", now) {
        CrewResolution::Resolved { id, others } => {
            assert_eq!(id, "run_new");
            assert_eq!(others, vec!["run_mid".to_string()]); // run_old is outside 30 days
        }
        CrewResolution::NotFound => panic!(),
    }
    assert!(matches!(resolve_crew(&cands, "amber-lake", now), CrewResolution::NotFound));
}
```
`runs.rs` test for `find_instance`: create a run in a temp `RunStore`, append a `StepResultRecord` with one `ItemResultRecord { codename: Some("jade-reef/numbat#2"), run_id: "run_U", transcript_path: "/u.jsonl", .. }`, and assert `find_instance(run_id, "jade-reef/numbat#2") == Some(("run_U", "/u.jsonl"))` and a miss returns `None`. Also write `<run>/sub/sub_A/transcript.jsonl` whose first line is a `RunStart` with `codename: Some("jade-reef/numbat#2>ferret#1")` and assert it resolves to `("sub_A", that path)`.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p rupu-cli crew_resolves; cargo test -p rupu-orchestrator find_instance`
Expected: compile errors.

- [ ] **Step 3: Implement**

```rust
pub fn resolve_crew(candidates: &[CrewCandidate], crew: &str, now: DateTime<Utc>) -> CrewResolution {
    let mut hits: Vec<&CrewCandidate> = candidates.iter().filter(|c| c.crew == crew).collect();
    hits.sort_by(|a, b| b.started_at.cmp(&a.started_at));
    let Some(first) = hits.first() else { return CrewResolution::NotFound };
    let cutoff = now - chrono::Duration::days(30);
    let others = hits[1..].iter().filter(|c| c.started_at >= cutoff).map(|c| c.id.clone()).collect();
    CrewResolution::Resolved { id: first.id.clone(), others }
}

pub fn ambiguity_note(crew: &str, chosen: &str, others: &[String]) -> String {
    format!("note: `{crew}` also named {} in the last 30 days; using the most recent ({chosen})", others.join(", "))
}
```
`RunStore::find_instance`: read `step_results.jsonl` (existing reader used by `load_step_results` or equivalent) and return the first `StepResultRecord` with `codename == Some(codename)` → `(rec.run_id, rec.transcript_path)`, else the first `ItemResultRecord` match → `(item.run_id, item.transcript_path)`; else scan `<root>/<run_id>/sub/*/transcript.jsonl` reading only the first line via `rupu_transcript::JsonlReader` and matching `RunStart.codename` → `(dir name, path)`.

Wire-up (order in each resolver: exact id → codename → id fragment; only try codename when `fragment.parse::<Codename>().is_ok()`):
- `resolve_run_fragment`: add `crew: String` to `RunCandidate` (`display_codename(r.codename.as_deref(), &r.id, None)`); if the fragment parses as a `Codename`, `resolve_crew` on its `crew`; on `Resolved { others }` non-empty, `eprintln!("{}", ambiguity_note(..))`. Return the workflow run id (commands that take a run id operate on the run; the member part is ignored here).
- `locate_transcript`: if the fragment parses and has segments, resolve the crew through `RunStore` as above, then `store.find_instance(run_id, fragment)` → open that transcript path; crew-only → the run id's transcript as today (`locate_transcript_exact`). Error text on miss: ``no run or agent named `{fragment}` (see `rupu workflow runs`)``.
- `read_session` / `resolve_session_fragment`: build `CrewCandidate`s from session records in scope (`crew` = the crew part of `display_codename(rec.codename…, &rec.session_id, Some(&rec.agent_name))`, `started_at` = `created_at`) and resolve the same way.

- [ ] **Step 4: Run tests**

Run: `cargo test -p rupu-cli && cargo test -p rupu-orchestrator`
Expected: PASS.

- [ ] **Step 5: Manual check**

```bash
cargo run -p rupu-cli -- workflow runs --limit 3
cargo run -p rupu-cli -- workflow show <a NAME from the list above>
cargo run -p rupu-cli -- transcript show <that NAME>
```
Expected: the run resolves by name; an older duplicate crew prints the `note:` line.

- [ ] **Step 6: Commit**

```bash
git add -A crates
git commit -m "feat(cli): accept codenames wherever a run, session or transcript id is accepted"
```

---

### Task 11: Docs + full verification

**Files:**
- Modify: `CLAUDE.md` (Crates list: add `rupu-codename`; Read-first: add the spec + this plan)
- Modify: `docs/superpowers/specs/2026-09-29-rupu-agent-codenames-design.md` (status line → "Plan 1 complete")

- [ ] **Step 1: CLAUDE.md**

Under `### Crates` add (alphabetical position after `rupu-cli`):
```markdown
- **`rupu-codename`** — leaf crate for human codenames (`cobalt-harbor/heron#412>lynx#3`): frozen word lists + FNV-1a hashing (`crew_for`, `role_word`, `derive_legacy`), the `Codename` grammar, crew tint / role badge palette, and `CrewNamer`/`SharedNamer` (per-run role allocation + instance counters, persisted to `<run_dir>/codenames.json`). The orchestrator's `codenames::RunNaming` walks static slots; names are minted once and stored on records, `RunStart`, and executor events (`AgentStarted` carries codename · agent · provider · model). Word lists are FROZEN — golden tests pin them.
```
Under `## Read first` add:
```markdown
- Agent codenames spec + Plan 1 (core + CLI): `docs/superpowers/specs/2026-09-29-rupu-agent-codenames-design.md`, `docs/superpowers/plans/2026-09-29-rupu-agent-codenames-plan-1-core-cli.md`
```

- [ ] **Step 2: Full verification**

```bash
cargo build --workspace --tests
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```
Expected: all green (compare against the baseline you measured before Task 1 — pre-existing failures, if any, must be unchanged and called out in the PR).

- [ ] **Step 3: End-to-end smoke**

```bash
cargo run -p rupu-cli -- run <any agent in .rupu/agents> --prompt "say hi"
cargo run -p rupu-cli -- workflow run <a small workflow in .rupu/workflows>
cargo run -p rupu-cli -- transcript show <the NAME printed in the header>
```
Expected: headers show the crew and member names; the transcript's system prompt contains the call-sign line; `~/.rupu/runs/<run_id>/events.jsonl` contains `agent_started` lines with `codename`, `agent`, `provider`, `model`.

- [ ] **Step 4: Commit**

```bash
git add CLAUDE.md docs
git commit -m "docs: rupu-codename crate + codenames plan 1"
```

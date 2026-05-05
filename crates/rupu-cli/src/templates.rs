//! Embedded templates for `rupu init --with-samples`.
//!
//! The manifest is the single source-of-truth for what ships in
//! `--with-samples`. Adding a template is two steps:
//!
//!   1. Drop the file under `crates/rupu-cli/templates/<dir>/<name>`.
//!   2. Add a line to `MANIFEST` below.
//!
//! `init_manifest_in_sync.rs` enforces both directions: every file
//! under templates/ appears in MANIFEST, and every MANIFEST entry
//! exists on disk.

/// One curated template: a target-relative path (always under
/// `.rupu/`) and the embedded file content.
pub struct Template {
    /// Path RELATIVE to the project root, e.g. `.rupu/agents/review-diff.md`.
    pub target_relpath: &'static str,
    /// Raw file bytes embedded at build time via `include_str!`.
    pub content: &'static str,
}

/// The curated set shipped by `rupu init --with-samples`.
///
/// Test fixtures (`sample-<provider>.md` etc.) are intentionally NOT
/// in this list — they live in the rupu repo's `.rupu/` for slice
/// B-1 / B-2 development and are not user-facing templates.
pub const MANIFEST: &[Template] = &[
    Template {
        target_relpath: ".rupu/agents/review-diff.md",
        content: include_str!("../templates/agents/review-diff.md"),
    },
    Template {
        target_relpath: ".rupu/agents/add-tests.md",
        content: include_str!("../templates/agents/add-tests.md"),
    },
    Template {
        target_relpath: ".rupu/agents/fix-bug.md",
        content: include_str!("../templates/agents/fix-bug.md"),
    },
    Template {
        target_relpath: ".rupu/agents/scaffold.md",
        content: include_str!("../templates/agents/scaffold.md"),
    },
    Template {
        target_relpath: ".rupu/agents/summarize-diff.md",
        content: include_str!("../templates/agents/summarize-diff.md"),
    },
    Template {
        target_relpath: ".rupu/agents/scm-pr-review.md",
        content: include_str!("../templates/agents/scm-pr-review.md"),
    },
    Template {
        target_relpath: ".rupu/workflows/investigate-then-fix.yaml",
        content: include_str!("../templates/workflows/investigate-then-fix.yaml"),
    },
];

/// Skeleton config.toml content. Created on every `rupu init`.
pub const CONFIG_SKELETON: &str = r#"# rupu project config — see https://github.com/Section9Labs/rupu/blob/main/docs/providers.md

# default_model = "claude-sonnet-4-6"

# [scm.default]
# platform = "github"
# owner = "<your-org>"
# repo = "<this-repo>"

# [issues.default]
# tracker = "github"
# project = "<your-org>/<this-repo>"
"#;

/// `.gitignore` line that rupu owns. Init appends this to an existing
/// `.gitignore` (or creates one) when missing.
pub const GITIGNORE_ENTRY: &str = ".rupu/transcripts/";

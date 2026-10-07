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
        target_relpath: ".rupu/agents/repo-investigator.md",
        content: include_str!("../templates/agents/repo-investigator.md"),
    },
    Template {
        target_relpath: ".rupu/agents/repo-implementer.md",
        content: include_str!("../templates/agents/repo-implementer.md"),
    },
    Template {
        target_relpath: ".rupu/agents/code-reviewer.md",
        content: include_str!("../templates/agents/code-reviewer.md"),
    },
    Template {
        target_relpath: ".rupu/agents/issue-understander.md",
        content: include_str!("../templates/agents/issue-understander.md"),
    },
    Template {
        target_relpath: ".rupu/agents/spec-writer.md",
        content: include_str!("../templates/agents/spec-writer.md"),
    },
    Template {
        target_relpath: ".rupu/agents/phase-planner.md",
        content: include_str!("../templates/agents/phase-planner.md"),
    },
    Template {
        target_relpath: ".rupu/agents/pr-author.md",
        content: include_str!("../templates/agents/pr-author.md"),
    },
    Template {
        target_relpath: ".rupu/agents/issue-commenter.md",
        content: include_str!("../templates/agents/issue-commenter.md"),
    },
    Template {
        target_relpath: ".rupu/agents/writer.md",
        content: include_str!("../templates/agents/writer.md"),
    },
    Template {
        target_relpath: ".rupu/agents/security-reviewer.md",
        content: include_str!("../templates/agents/security-reviewer.md"),
    },
    Template {
        target_relpath: ".rupu/agents/performance-reviewer.md",
        content: include_str!("../templates/agents/performance-reviewer.md"),
    },
    Template {
        target_relpath: ".rupu/agents/maintainability-reviewer.md",
        content: include_str!("../templates/agents/maintainability-reviewer.md"),
    },
    Template {
        target_relpath: ".rupu/agents/finding-fixer.md",
        content: include_str!("../templates/agents/finding-fixer.md"),
    },
    Template {
        target_relpath: ".rupu/workflows/investigate-then-fix.yaml",
        content: include_str!("../templates/workflows/investigate-then-fix.yaml"),
    },
    Template {
        target_relpath: ".rupu/workflows/quick-bugfix.yaml",
        content: include_str!("../templates/workflows/quick-bugfix.yaml"),
    },
    Template {
        target_relpath: ".rupu/workflows/review-changed-files.yaml",
        content: include_str!("../templates/workflows/review-changed-files.yaml"),
    },
    Template {
        target_relpath: ".rupu/workflows/code-review-panel.yaml",
        content: include_str!("../templates/workflows/code-review-panel.yaml"),
    },
    Template {
        target_relpath: ".rupu/workflows/issue-to-spec-and-plan.yaml",
        content: include_str!("../templates/workflows/issue-to-spec-and-plan.yaml"),
    },
    Template {
        target_relpath: ".rupu/workflows/phase-delivery-cycle.yaml",
        content: include_str!("../templates/workflows/phase-delivery-cycle.yaml"),
    },
    Template {
        target_relpath: ".rupu/workflows/issue-supervisor-dispatch.yaml",
        content: include_str!("../templates/workflows/issue-supervisor-dispatch.yaml"),
    },
    Template {
        target_relpath: ".rupu/contracts/autoflow_outcome_v1.json",
        content: include_str!("../templates/contracts/autoflow_outcome_v1.json"),
    },
    Template {
        target_relpath: ".rupu/contracts/workflow_dispatch_v1.json",
        content: include_str!("../templates/contracts/workflow_dispatch_v1.json"),
    },
    Template {
        target_relpath: ".rupu/contracts/phase_plan_v1.json",
        content: include_str!("../templates/contracts/phase_plan_v1.json"),
    },
    Template {
        target_relpath: ".rupu/contracts/review_packet_v1.json",
        content: include_str!("../templates/contracts/review_packet_v1.json"),
    },
];

/// The stock security-assessment fleet shipped by `rupu fleet install`.
///
/// Unlike [`MANIFEST`] (project dev/code samples, materialized into a
/// project's `.rupu/`), the fleet is the generic agents + workflows the
/// built-in engagement profiles name in their `[bundle]`. It is installed
/// into the GLOBAL rupu root (`~/.rupu`), so any engagement — in any
/// project — can draw on it without per-engagement hand-authoring.
///
/// `target_relpath` here is therefore relative to the GLOBAL root
/// (`agents/<name>.md`, `workflows/<id>.yaml`), NOT `.rupu/`-prefixed, so
/// it lands exactly where `rupu_agent::load_agents` /
/// `rupu_orchestrator::list_workflow_summaries` read it. The on-disk
/// source lives under `crates/rupu-cli/templates/fleet/<target_relpath>`.
///
/// Adding a fleet member is the same two steps as [`MANIFEST`]: drop the
/// file under `templates/fleet/<dir>/<name>` and add a line here.
/// `init_manifest_in_sync.rs` enforces both directions, and
/// `fleet_bundle_coverage.rs` enforces that every built-in profile's
/// `[bundle]` names a fleet member that exists here.
pub const FLEET_MANIFEST: &[Template] = &[
    Template {
        target_relpath: "agents/assessment-lead.md",
        content: include_str!("../templates/fleet/agents/assessment-lead.md"),
    },
    Template {
        target_relpath: "agents/recon.md",
        content: include_str!("../templates/fleet/agents/recon.md"),
    },
    Template {
        target_relpath: "agents/service-analyst.md",
        content: include_str!("../templates/fleet/agents/service-analyst.md"),
    },
    Template {
        target_relpath: "agents/exploit-verifier.md",
        content: include_str!("../templates/fleet/agents/exploit-verifier.md"),
    },
    Template {
        target_relpath: "agents/crawler.md",
        content: include_str!("../templates/fleet/agents/crawler.md"),
    },
    Template {
        target_relpath: "agents/appsec-tester.md",
        content: include_str!("../templates/fleet/agents/appsec-tester.md"),
    },
    Template {
        target_relpath: "agents/api-tester.md",
        content: include_str!("../templates/fleet/agents/api-tester.md"),
    },
    Template {
        target_relpath: "agents/code-auditor.md",
        content: include_str!("../templates/fleet/agents/code-auditor.md"),
    },
    Template {
        target_relpath: "agents/sca-auditor.md",
        content: include_str!("../templates/fleet/agents/sca-auditor.md"),
    },
    Template {
        target_relpath: "agents/secret-scanner.md",
        content: include_str!("../templates/fleet/agents/secret-scanner.md"),
    },
    Template {
        target_relpath: "agents/iac-reviewer.md",
        content: include_str!("../templates/fleet/agents/iac-reviewer.md"),
    },
    Template {
        target_relpath: "agents/binary-analyst.md",
        content: include_str!("../templates/fleet/agents/binary-analyst.md"),
    },
    Template {
        target_relpath: "agents/firmware-analyst.md",
        content: include_str!("../templates/fleet/agents/firmware-analyst.md"),
    },
    Template {
        target_relpath: "agents/cloud-auditor.md",
        content: include_str!("../templates/fleet/agents/cloud-auditor.md"),
    },
    Template {
        target_relpath: "agents/container-scanner.md",
        content: include_str!("../templates/fleet/agents/container-scanner.md"),
    },
    Template {
        target_relpath: "agents/threat-modeler.md",
        content: include_str!("../templates/fleet/agents/threat-modeler.md"),
    },
    Template {
        target_relpath: "agents/mobile-analyst.md",
        content: include_str!("../templates/fleet/agents/mobile-analyst.md"),
    },
    Template {
        target_relpath: "agents/redteam-operator.md",
        content: include_str!("../templates/fleet/agents/redteam-operator.md"),
    },
    Template {
        target_relpath: "workflows/network-assessment.yaml",
        content: include_str!("../templates/fleet/workflows/network-assessment.yaml"),
    },
    Template {
        target_relpath: "workflows/web-assessment.yaml",
        content: include_str!("../templates/fleet/workflows/web-assessment.yaml"),
    },
    Template {
        target_relpath: "workflows/api-assessment.yaml",
        content: include_str!("../templates/fleet/workflows/api-assessment.yaml"),
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

# [autoflow]
# enabled = true
# repo = "github:<your-org>/<this-repo>"
# permission_mode = "bypass"
# strict_templates = true
"#;

/// `.gitignore` lines that rupu owns. Init appends any of these missing
/// from an existing `.gitignore` (or creates one) when missing.
///
/// `.rupu/netflow/` MUST be here alongside `.rupu/transcripts/` — the
/// netflow ledger (one `<project>/.rupu/netflow/<run_id>.jsonl` per run,
/// see `NetflowPaths::for_run`; `flows.jsonl` was the pre-per-run shared
/// filename and no longer exists) records every host, IP, path and
/// timing rupu contacted on the user's behalf. Without this entry,
/// `git add .` in a freshly-`init`'d project commits that record
/// straight into the user's repo. `cmd::init::ensure_netflow_dir` also
/// creates this directory itself (with its own self-ignoring
/// `.gitignore`, belt-and-suspenders with this entry) — see that
/// function's doc comment for why that's a deliberate, explicit opt-in
/// and not auto-creation-on-write.
pub const GITIGNORE_ENTRIES: &[&str] = &[".rupu/transcripts/", ".rupu/netflow/"];

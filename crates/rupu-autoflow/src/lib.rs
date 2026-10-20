//! Autoflows' read side and claim repair: which autoflows are visible from
//! a directory and how one resolves for an issue, the reports `rupu
//! autoflow show | explain | doctor | wakes` print with `--format json`,
//! `rupu autoflow repair`, and the checks `rupu autoflow run` makes before
//! it starts a cycle. The control plane's `/api/autoflows/*` endpoints call
//! the same functions, so the two can never disagree.
//!
//! What stays in `rupu-cli`: running a cycle (`autoflow run | tick |
//! serve`), the live views, and the human renderings of these reports.
//!
//! Visibility is relative to a directory: an autoflow is visible when it
//! lives in that directory's project layer (`<project>/.rupu/workflows`) or
//! the global one, or in a tracked repo's checkout. The CLI passes its
//! working directory; the control plane passes the directory `cp serve`
//! serves.
//!
//! Errors are `anyhow` chains, as the CLI prints them; a refusal the caller
//! should see by its kind (a bad argument, nothing by that name, a state
//! that forbids it) is a [`Refusal`] somewhere in the chain
//! ([`refusal_of`]).

pub mod claims;
pub mod format;
pub mod history;
pub mod paths;
pub mod refs;
mod refusal;
pub mod reports;
pub mod run_summary;
pub mod visible;
pub mod wakes;

pub use refusal::{refusal_of, Refusal};
pub use reports::{
    doctor_report, explain_report, repair_claim, run_precheck, show_entry, show_report,
    AutoflowDoctorReport, AutoflowExplainReport, AutoflowShowReport, RepairOutcome,
};
pub use visible::{ResolvedAutoflowWorkflow, VisibleAutoflowWorkflow};
pub use wakes::{wakes_report, AutoflowWakesReport};

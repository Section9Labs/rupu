//! Structured finding reports.
//!
//! A finding under the `full` profile carries a [`FindingReport`]: the whole
//! assessment write-up as typed data. Every presentation (UI sections,
//! Markdown/HTML/PDF exports) is generated from it, so the agent writes it
//! exactly once. Design: docs/superpowers/specs/2026-09-29-rupu-finding-reports-design.md

pub mod profile;
pub mod types;

pub use profile::FindingProfile;
pub use types::*;

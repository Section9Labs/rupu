//! rupu coverage harness — exhaustive-coverage ledgers, concern catalogs, and agent tools.

#![deny(clippy::all)]
#![forbid(unsafe_code)]

pub mod catalog;

pub use catalog::{Concern, Severity, Template, TouchStrength};

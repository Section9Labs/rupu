//! Engagement profiles: the data-driven packages that make the finding and
//! coverage layer agnostic to the *type of asset*. A profile declares asset
//! kinds (with their coordinates), the evidence blocks and taxonomies it uses,
//! a completeness checklist, a coverage depth ladder, and a launcher bundle.
//!
//! The core owns only the typed primitives (see [`crate::asset`] and
//! [`crate::report`]); a new engagement type is authored as data, not code.

pub mod predicate;

pub use predicate::{evaluate, CompletenessCheck, Predicate, PredicateError};

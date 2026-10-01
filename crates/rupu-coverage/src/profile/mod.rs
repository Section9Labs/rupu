//! Engagement profiles: data-driven asset families layered on the primitives.
pub mod loader;
pub mod package;
pub mod predicate;

pub use loader::{discover, expand_includes, LoadError};
pub use package::{parse_profile, AssetKindDef, Bundle, CoverageSpec, EngagementProfile, ProfileError};
pub use predicate::{score, CompletenessCheck, Predicate, PredicateError};

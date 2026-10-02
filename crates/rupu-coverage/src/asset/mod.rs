//! The asset model: typed locator coordinates, the asset graph, and its store.
//!
//! A finding is *about an asset*, and the asset has a profile-namespaced
//! `kind`. The kind drives everything the finding/coverage layer used to
//! hardcode for "code": the locator shape, evidence shape, taxonomy, and what
//! coverage counts. See the engagement-profiles spec.

pub mod coordinate;

pub use coordinate::{Coordinate, Locator, Proto};

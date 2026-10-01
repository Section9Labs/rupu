//! Asset model: the profile-namespaced subjects a finding is about.
pub mod coordinate;
pub mod graph;
pub mod types;

pub use coordinate::{Coordinate, Locator, Proto};
pub use graph::AssetGraph;
pub use types::{Asset, AssetId};

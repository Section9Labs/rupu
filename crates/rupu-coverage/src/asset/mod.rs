//! Asset model: the profile-namespaced subjects a finding is about.
pub mod coordinate;
pub mod graph;
pub mod store;
pub mod types;

pub use coordinate::{Coordinate, Locator, Proto};
pub use graph::AssetGraph;
pub use store::{append_asset, read_asset_graph};
pub use types::{Asset, AssetId};

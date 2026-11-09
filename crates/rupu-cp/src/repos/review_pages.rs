//! Internal, typed read port. Provider cursors and nested targets never cross HTTP.
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    pub account: String,
    /// Random connector lifetime plus credential replacement epoch; never secret-derived.
    pub connector_epoch: String,
    pub api_profile: String,
    pub provider_version: Option<String>,
    pub repository_id: String,
    pub number: u32,
    pub branch: String,
    pub head: String,
    pub base: String,
    pub start: Option<String>,
    pub pipeline: Option<String>,
    pub file_count: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Request {
    pub section: String,
    pub after: Option<String>,
    pub parent: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Target {
    Provider(Request),
    /// Notes already captured within one bounded provider response.
    Captured {
        scope: String,
        items: Vec<Item>,
    },
    /// Exact captured bytes, not offsets into a future mutable provider read.
    Text {
        id: String,
        text: String,
        offset: usize,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Child {
    pub field: String,
    pub section: String,
    pub target: Target,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Item {
    pub id: String,
    pub value: Value,
    pub children: Vec<Child>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Page {
    pub identity: Identity,
    pub scope: String,
    pub items: Vec<Item>,
    pub next: Option<Request>,
    pub provider_exhausted: bool,
    pub omissions: Vec<String>,
    /// Sum of bounded, decoded response bytes, including before/after identity.
    pub input_bytes: usize,
    pub page_version: Option<String>,
}

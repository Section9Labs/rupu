//! The one-line query language behind every filterable list: the grammar,
//! generic over a field registry ([`grammar`]), time values ([`time`]) and a
//! row matcher with facets ([`rows`]). A leaf crate: the findings registry
//! (`rupu-coverage`) and the control plane's list registries (`rupu-cp`)
//! build on it.

pub mod grammar;
pub mod rows;
pub mod time;

pub use grammar::{
    field, parse_query, ErrorCode, FieldDef, FieldKind, Normalize, Op, ParseError, ParsedQuery,
    Term, TEXT_KEY,
};
pub use rows::{defs, facets, matches, Facets, Getter, MatchMode, RowField, Unreportable};
pub use time::TimeValue;

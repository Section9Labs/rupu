//! Pure renderers for finding reports: Markdown, HTML, and PDF (via Typst).
//! No I/O — callers supply findings, metadata and the timestamp.

#![deny(clippy::all)]
#![forbid(unsafe_code)]

pub mod blocks;
pub mod markdown;
pub mod model;
pub mod number;
pub mod prose;
mod text;

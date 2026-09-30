//! Pure renderers for finding reports: Markdown, HTML, and PDF (via Typst).
//! No I/O — callers supply findings, metadata and the timestamp.

#![deny(clippy::all)]
#![forbid(unsafe_code)]

pub mod blocks;
pub mod html;
pub mod markdown;
pub mod model;
pub mod number;
pub mod pdf;
pub mod prose;
pub mod render;
mod text;
pub mod typst_doc;

pub use render::{render_finding, render_project, render_split_zip, ExportError, Format};

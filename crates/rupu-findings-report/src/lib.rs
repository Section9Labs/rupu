//! Pure renderers for finding reports: Markdown, HTML, and PDF (via Typst).
//! No I/O — callers supply findings, metadata and the timestamp.
//!
//! PDF needs the `pdf` cargo feature (on by default); it embeds Typst and its
//! fonts. Built without it, everything else works and `Format::Pdf` returns
//! [`ExportError::PdfUnavailable`].

#![deny(clippy::all)]
#![forbid(unsafe_code)]

pub mod blocks;
pub mod html;
pub mod markdown;
pub mod model;
pub mod number;
#[cfg(feature = "pdf")]
pub mod pdf;
pub mod prose;
pub mod render;
pub mod select;
mod text;
pub mod typst_doc;

pub use render::{render_finding, render_project, render_split_zip, ExportError, Format};

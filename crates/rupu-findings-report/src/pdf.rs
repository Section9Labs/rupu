//! PDF rendering: compiles our own Typst markup in-process with the fonts
//! bundled by `typst-assets`. The only source is the generated document;
//! every other file access is refused.

use std::sync::LazyLock;
use typst::diag::{FileError, FileResult, SourceDiagnostic};
use typst::foundations::{Bytes, Datetime, Duration};
use typst::syntax::{FileId, Source};
use typst::text::{Font, FontBook};
use typst::utils::LazyHash;
use typst::{Library, LibraryExt, World};
use typst_layout::PagedDocument;

use crate::render::ExportError;

struct Fonts {
    book: LazyHash<FontBook>,
    fonts: Vec<Font>,
}

/// Parsing the bundled fonts is the slow part of a compile, so it happens
/// once per process rather than once per report.
static FONTS: LazyLock<Fonts> = LazyLock::new(|| {
    let fonts: Vec<Font> = typst_assets::fonts()
        .flat_map(|data| Font::iter(Bytes::new(data)))
        .collect();
    Fonts {
        book: LazyHash::new(FontBook::from_fonts(&fonts)),
        fonts,
    }
});

static LIBRARY: LazyLock<LazyHash<Library>> = LazyLock::new(|| LazyHash::new(Library::default()));

struct ReportWorld {
    main: Source,
}

impl World for ReportWorld {
    fn library(&self) -> &LazyHash<Library> {
        &LIBRARY
    }

    fn book(&self) -> &LazyHash<FontBook> {
        &FONTS.book
    }

    fn main(&self) -> FileId {
        self.main.id()
    }

    fn source(&self, id: FileId) -> FileResult<Source> {
        if id == self.main.id() {
            Ok(self.main.clone())
        } else {
            Err(FileError::AccessDenied)
        }
    }

    fn file(&self, _id: FileId) -> FileResult<Bytes> {
        Err(FileError::AccessDenied)
    }

    fn font(&self, index: usize) -> Option<Font> {
        FONTS.fonts.get(index).cloned()
    }

    fn today(&self, _offset: Option<Duration>) -> Option<Datetime> {
        // Reports never print "today"; a fixed date keeps output deterministic.
        Datetime::from_ymd(2000, 1, 1)
    }
}

fn typst_error(
    diags: impl IntoIterator<Item = impl std::borrow::Borrow<SourceDiagnostic>>,
) -> ExportError {
    let msgs: Vec<String> = diags
        .into_iter()
        .map(|d| d.borrow().message.to_string())
        .collect();
    ExportError::Typst(msgs.join("; "))
}

/// How many renders an unused cache entry survives (`comemo::evict`'s
/// `max_age`). Measured on 500 distinct project reports in a release build:
/// 10 keeps the resident set flat within a few MB after a ~50 MB warm-up,
/// 3 sits at about +15 MB, and 0 (clear everything) is fully flat but ~30%
/// slower per render (10 ms vs 7 ms). Raise it only for workloads that
/// re-render near-identical documents back to back.
const CACHE_MAX_AGE: usize = 10;

/// Evicts Typst's global memoization cache when a render ends, on success and
/// failure alike (it is a drop guard so no early return skips it).
///
/// comemo's cache is process-global and only shrinks when told to. Every
/// report has different content, so little of it is ever reused across
/// reports, and a long-running `cp serve` would otherwise grow by about a
/// megabyte per export forever.
struct EvictCacheOnDrop;

impl Drop for EvictCacheOnDrop {
    fn drop(&mut self) {
        typst::comemo::evict(CACHE_MAX_AGE);
    }
}

/// Compile `markup` (a complete Typst document) to PDF bytes. A compile error
/// is returned, never panicked on.
///
/// Only the fonts bundled with `typst-assets` are available (Libertinus Serif,
/// New Computer Modern, DejaVu Sans Mono), and none of them covers CJK or
/// emoji: those characters are not lost from the file, but they render as
/// missing-glyph boxes. Use the Markdown or HTML export where the reader's own
/// fonts matter.
pub fn render_pdf(markup: String) -> Result<Vec<u8>, ExportError> {
    let _evict = EvictCacheOnDrop;
    let world = ReportWorld {
        main: Source::detached(markup),
    };
    let doc: PagedDocument = typst::compile(&world).output.map_err(typst_error)?;
    typst_pdf::pdf(&doc, &typst_pdf::PdfOptions::default()).map_err(typst_error)
}

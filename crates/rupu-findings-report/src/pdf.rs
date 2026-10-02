//! PDF rendering: compiles our own Typst markup in-process with the fonts
//! bundled by `typst-assets`. The only source is the generated document, and
//! the only files are the images it carries in memory
//! ([`TypstDoc::files`]); every other file access is refused.

use std::sync::LazyLock;
use typst::diag::{FileError, FileResult, SourceDiagnostic};
use typst::foundations::{Bytes, Datetime, Duration};
use typst::syntax::{FileId, Source, VirtualRoot};
use typst::text::{Font, FontBook};
use typst::utils::LazyHash;
use typst::{Library, LibraryExt, World};
use typst_layout::PagedDocument;

use crate::render::ExportError;
use crate::typst_doc::TypstDoc;

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
    files: std::collections::BTreeMap<String, std::sync::Arc<[u8]>>,
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

    /// Only the document's own in-memory images, by their exact virtual
    /// path in the project root; nothing on disk.
    fn file(&self, id: FileId) -> FileResult<Bytes> {
        if *id.root() != VirtualRoot::Project {
            return Err(FileError::AccessDenied);
        }
        self.files
            .get(id.vpath().get_with_slash())
            .map(|b| Bytes::new(std::sync::Arc::clone(b)))
            .ok_or(FileError::AccessDenied)
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
/// `max_age`); 0 clears the whole cache after every render. Measured on 500
/// distinct project reports in a release build: 10 leaves the resident set
/// about 50 MB above baseline and still creeping (~7 KB per render), 3 sits at
/// about +15 MB, and 0 is flat (~0.4 KB per render) for about 3 ms more per
/// render (10 ms vs 7 ms). Reports rarely share work, so a long-running
/// `cp serve` takes flat memory over the small speed-up.
const CACHE_MAX_AGE: usize = 0;

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

/// Compile `doc` (a complete Typst document: markup, or markup with its
/// images) to PDF bytes. A compile error — an image Typst cannot decode
/// included — is returned, never panicked on.
///
/// Only the fonts bundled with `typst-assets` are available (Libertinus Serif,
/// New Computer Modern, DejaVu Sans Mono), and none of them covers CJK or
/// emoji: those characters are not lost from the file, but they render as
/// missing-glyph boxes. Use the Markdown or HTML export where the reader's own
/// fonts matter.
pub fn render_pdf(doc: impl Into<TypstDoc>) -> Result<Vec<u8>, ExportError> {
    let _evict = EvictCacheOnDrop;
    let doc = doc.into();
    let world = ReportWorld {
        main: Source::detached(doc.markup),
        files: doc.files,
    };
    let doc: PagedDocument = typst::compile(&world).output.map_err(typst_error)?;
    typst_pdf::pdf(&doc, &typst_pdf::PdfOptions::default()).map_err(typst_error)
}

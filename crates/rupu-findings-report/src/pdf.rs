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

/// Compile `markup` (a complete Typst document) to PDF bytes. A compile error
/// is returned, never panicked on.
pub fn render_pdf(markup: String) -> Result<Vec<u8>, ExportError> {
    let world = ReportWorld {
        main: Source::detached(markup),
    };
    let doc: PagedDocument = typst::compile(&world).output.map_err(typst_error)?;
    typst_pdf::pdf(&doc, &typst_pdf::PdfOptions::default()).map_err(typst_error)
}

//! Evidence-block files (`image` / `hexdump` / `pcap_ref`) in every export
//! format: an image from the local store is embedded (HTML, PDF) or
//! referenced (Markdown); anything not local, too big or not a raster image is
//! named by path, sha256 and size with a note saying it was not embedded.

use crate::common::*;
use base64::Engine as _;
use rupu_coverage::report::{sha256_reader, ArtifactRef, ArtifactStorage, EvidenceBlock};
use rupu_coverage::Severity;
use rupu_findings_report::blocks::{finding_blocks, Block, MAX_EMBED_BYTES};
use rupu_findings_report::model::ExportFinding;
use rupu_findings_report::number::number_map;
use rupu_findings_report::{render_finding, Blobs, Format};
use std::cell::Cell;
use std::collections::HashMap;

/// A 1×1 RGB PNG, built for these tests (IHDR, one zlib IDAT row, IEND).
const PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53,
    0xde, 0x00, 0x00, 0x00, 0x0c, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x38, 0x63, 0x9c, 0x06,
    0x00, 0x03, 0x34, 0x01, 0x66, 0x38, 0x86, 0xe0, 0xac, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e,
    0x44, 0xae, 0x42, 0x60, 0x82,
];

const SVG: &[u8] = b"<svg xmlns=\"http://www.w3.org/2000/svg\"><script>x()</script></svg>";

fn sha(bytes: &[u8]) -> String {
    sha256_reader(&mut &bytes[..]).unwrap()
}

/// A file recorded as copied into the local store.
fn copied(path: &str, bytes: &[u8]) -> ArtifactRef {
    ArtifactRef {
        path: path.into(),
        sha256: sha(bytes),
        size: bytes.len() as u64,
        kind: None,
        stored: Some(ArtifactStorage::Copied),
        host: None,
    }
}

fn finding_with(blocks: Vec<EvidenceBlock>) -> ExportFinding {
    let mut report = full_report();
    report.blocks = blocks;
    numbered(vec![input(
        "notebin",
        None,
        full_record("fnd_ev", Severity::High, report),
    )])
    .remove(0)
}

fn image(artifact: ArtifactRef, caption: Option<&str>) -> EvidenceBlock {
    EvidenceBlock::Image {
        artifact,
        caption: caption.map(str::to_string),
    }
}

/// A store holding `blobs`, read the way `ArtifactStore::read_verified`
/// reads: nothing over `max`. Counts the reads.
struct FakeStore {
    blobs: HashMap<String, Vec<u8>>,
    reads: Cell<usize>,
}

impl FakeStore {
    fn of(files: &[&[u8]]) -> Self {
        FakeStore {
            blobs: files.iter().map(|b| (sha(b), b.to_vec())).collect(),
            reads: Cell::new(0),
        }
    }

    fn read(&self, sha256: &str, max: u64) -> Option<Vec<u8>> {
        self.reads.set(self.reads.get() + 1);
        self.blobs
            .get(sha256)
            .filter(|b| b.len() as u64 <= max)
            .cloned()
    }
}

fn render(f: &ExportFinding, store: &FakeStore, fmt: Format) -> Vec<u8> {
    let read = |s: &str, max: u64| store.read(s, max);
    render_finding(
        f,
        &number_map(std::slice::from_ref(f)),
        fmt,
        Blobs::new(&read),
    )
    .unwrap()
}

fn text(f: &ExportFinding, store: &FakeStore, fmt: Format) -> String {
    String::from_utf8(render(f, store, fmt)).unwrap()
}

fn data_uri(mime: &str, bytes: &[u8]) -> String {
    format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    )
}

#[test]
fn a_local_raster_image_is_embedded_in_html_with_its_caption_escaped() {
    let f = finding_with(vec![image(
        copied("out/shot.png", PNG),
        Some("Note <b>leaks</b> & \"more\""),
    )]);
    let store = FakeStore::of(&[PNG]);
    let html = text(&f, &store, Format::Html);
    assert!(html.contains(&data_uri("image/png", PNG)), "{html}");
    assert!(
        html.contains(
            "alt=\"Note &lt;b&gt;leaks&lt;/b&gt; &amp; &quot;more&quot;\"><figcaption>Note &lt;b&gt;leaks&lt;/b&gt; &amp; &quot;more&quot;</figcaption>"
        ),
        "{html}"
    );
    assert!(!html.contains("<b>leaks"), "{html}");
    // The file is still named under the image.
    assert!(html.contains("out/shot.png"), "{html}");
    assert!(html.contains(&sha(PNG)), "{html}");
    assert!(html.contains(&format!("{} bytes", PNG.len())), "{html}");
    // The page still loads nothing but `data:` images.
    assert!(html.contains("img-src data:"), "{html}");
    assert!(
        !html.contains("http://") && !html.contains("https://"),
        "{html}"
    );
}

#[test]
fn the_image_type_comes_from_the_bytes_and_svg_is_never_embedded() {
    let gif: &[u8] = b"GIF89a\x01\x00\x01\x00\x80\x00\x00\xff\xff\xff\x00\x00\x00!\xf9\x04\x01\x00\x00\x00\x00,\x00\x00\x00\x00\x01\x00\x01\x00\x00\x02\x02D\x01\x00;";
    let f = finding_with(vec![
        // A GIF recorded under a .png name is embedded as the GIF it is.
        image(copied("out/misnamed.png", gif), Some("gif")),
        image(copied("out/diagram.svg", SVG), Some("svg")),
    ]);
    let store = FakeStore::of(&[gif, SVG]);
    let html = text(&f, &store, Format::Html);
    assert!(html.contains(&data_uri("image/gif", gif)), "{html}");
    assert!(!html.contains("image/svg"), "{html}");
    assert!(!html.contains("<svg"), "{html}");
    assert!(
        html.contains("not embedded: the file is not a PNG, JPEG, GIF or WebP image"),
        "{html}"
    );
}

#[test]
fn files_that_are_not_local_are_named_and_never_read() {
    let mut on_host = copied("out/remote.png", PNG);
    on_host.stored = Some(ArtifactStorage::External);
    on_host.host = Some("node-7".into());
    let mut external = copied("out/huge.png", PNG);
    external.stored = Some(ArtifactStorage::External);
    let mut unrecorded = copied("out/legacy.png", PNG);
    unrecorded.stored = None;
    let mut bad_sha = copied("out/bad.png", PNG);
    bad_sha.sha256 = "../../etc/passwd".into();
    let mut over_cap = copied("out/big.png", PNG);
    over_cap.size = MAX_EMBED_BYTES + 1;
    let f = finding_with(vec![
        image(on_host, Some("remote")),
        image(external, Some("external")),
        image(unrecorded, Some("legacy")),
        image(bad_sha, Some("bad")),
        image(over_cap, Some("big")),
    ]);
    let store = FakeStore::of(&[PNG]);
    let html = text(&f, &store, Format::Html);
    assert_eq!(store.reads.get(), 0, "nothing was looked up");
    assert!(!html.contains("data:image"), "{html}");
    for why in [
        "not embedded: the file is on host",
        "not embedded: the file was not copied into the artifact store",
        "not embedded: no valid sha256 was recorded for the file",
        &format!(
            "not embedded: {} bytes is over the 4 MiB embed limit",
            MAX_EMBED_BYTES + 1
        ),
    ] {
        assert!(html.contains(why), "{why} missing from {html}");
    }
    assert!(html.contains("node-7"), "{html}");
    for path in [
        "out/remote.png",
        "out/huge.png",
        "out/legacy.png",
        "out/big.png",
    ] {
        assert!(html.contains(path), "{path} missing");
    }
}

#[test]
fn a_copied_file_missing_from_the_store_or_no_store_at_all_is_said_so() {
    let f = finding_with(vec![image(copied("out/gone.png", PNG), None)]);
    let empty = FakeStore::of(&[]);
    let md = text(&f, &empty, Format::Markdown);
    assert_eq!(empty.reads.get(), 1);
    assert!(
        md.contains("not embedded: the file is not in this machine's artifact store"),
        "{md}"
    );
    assert!(
        !md.contains("!["),
        "no image reference for a file not here: {md}"
    );

    let none = String::from_utf8(
        render_finding(
            &f,
            &number_map(std::slice::from_ref(&f)),
            Format::Html,
            Blobs::NONE,
        )
        .unwrap(),
    )
    .unwrap();
    assert!(
        none.contains("not embedded: no artifact store was available to this export"),
        "{none}"
    );
}

#[test]
fn markdown_references_the_image_by_path_and_inlines_no_bytes() {
    let f = finding_with(vec![image(
        copied("out/shot <1>.png", PNG),
        Some("Leaked *note* ![x](https://example.invalid/t.png)"),
    )]);
    let store = FakeStore::of(&[PNG]);
    let md = text(&f, &store, Format::Markdown);
    assert!(!md.contains("base64"), "{md}");
    // The caption is literal text: no emphasis, no nested image.
    let caption = r"Leaked \*note\* \!\[x\]\(https\:\/\/example\.invalid\/t\.png\)";
    assert!(
        md.contains(&format!("![{caption}](<out/shot \\<1\\>.png>)")),
        "{md}"
    );
    assert!(md.contains(&format!("_{caption}_")), "{md}");
    assert!(md.contains(&sha(PNG)), "{md}");
}

#[test]
fn markdown_never_turns_a_url_like_path_into_an_image_reference() {
    for path in [
        "https://example.invalid/x.png",
        "//example.invalid/x.png",
        r"\\fileserver\share\x.png",
        "data:image/png;base64,AAAA",
    ] {
        let f = finding_with(vec![image(copied(path, PNG), Some("cap"))]);
        let md = text(&f, &FakeStore::of(&[PNG]), Format::Markdown);
        assert!(!md.contains("!["), "{path}: {md}");
        assert!(md.contains("_cap_"), "{path}: {md}");
    }
    // A Windows drive is a path.
    let f = finding_with(vec![image(copied(r"C:\shots\x.png", PNG), Some("cap"))]);
    let md = text(&f, &FakeStore::of(&[PNG]), Format::Markdown);
    assert!(md.contains(r"![cap](<C:\\shots\\x.png>)"), "{md}");
}

#[test]
fn a_hexdump_shows_its_rendered_text_and_an_exact_64_bit_base() {
    let dump = b"\x7fELF\x02\x01\x01".to_vec();
    let f = finding_with(vec![
        EvidenceBlock::Hexdump {
            // Above 2^53: a float would round it to ...000.
            base: 0x1000_0000_0000_0001,
            artifact: copied("out/dump.bin", &dump),
            rendered: Some("00000000  7f 45 4c 46 02 01 01  |.ELF...|".into()),
        },
        EvidenceBlock::Hexdump {
            base: u64::MAX,
            artifact: copied("out/tail.bin", &dump),
            rendered: None,
        },
    ]);
    let store = FakeStore::of(&[&dump]);
    let md = text(&f, &store, Format::Markdown);
    assert_eq!(store.reads.get(), 0, "a hexdump's file is never read");
    assert!(md.contains("(base 0x1000000000000001)"), "{md}");
    assert!(md.contains("(base 0xffffffffffffffff)"), "{md}");
    assert!(
        md.contains("```\n00000000  7f 45 4c 46 02 01 01  |.ELF...|\n```"),
        "{md}"
    );
    assert!(md.contains("No rendered dump was recorded"), "{md}");
    assert!(md.contains(&sha(&dump)), "{md}");
    assert!(md.contains("`out/dump.bin`"), "{md}");

    let html = text(&f, &store, Format::Html);
    assert!(
        html.contains("<pre><code>00000000  7f 45 4c 46 02 01 01  |.ELF...|</code></pre>"),
        "{html}"
    );
    assert!(html.contains("0x1000000000000001"), "{html}");
}

#[test]
fn a_pcap_ref_shows_its_summary_and_its_file() {
    let pcap = b"\xd4\xc3\xb2\xa1 pretend capture".to_vec();
    let mut on_host = copied("caps/login.pcap", &pcap);
    on_host.host = Some("sensor-2".into());
    on_host.stored = Some(ArtifactStorage::External);
    let f = finding_with(vec![EvidenceBlock::PcapRef {
        artifact: on_host,
        summary: "The login POST and its 302, in clear text.".into(),
    }]);
    let store = FakeStore::of(&[&pcap]);
    let md = text(&f, &store, Format::Markdown);
    assert_eq!(store.reads.get(), 0);
    assert!(
        md.contains("**Packet capture** — `caps/login.pcap`"),
        "{md}"
    );
    assert!(md.contains(&sha(&pcap)), "{md}");
    assert!(md.contains(&format!("{} bytes", pcap.len())), "{md}");
    assert!(md.contains("on host `sensor-2`"), "{md}");
    assert!(
        md.contains("The login POST and its 302, in clear text."),
        "{md}"
    );
}

#[test]
fn the_image_block_carries_the_bytes_and_a_facts_line_follows() {
    let f = finding_with(vec![image(copied("out/shot.png", PNG), None)]);
    let store = FakeStore::of(&[PNG]);
    let read = |s: &str, max: u64| store.read(s, max);
    let blocks = finding_blocks(&f, &HashMap::new(), Blobs::new(&read));
    let i = blocks
        .iter()
        .position(|b| matches!(b, Block::Image { .. }))
        .expect("an image block");
    match &blocks[i] {
        Block::Image {
            caption,
            path,
            mime,
            bytes,
            ..
        } => {
            assert_eq!(caption, "Image");
            assert_eq!(path, "out/shot.png");
            assert_eq!(*mime, "image/png");
            assert_eq!(&bytes[..], PNG);
        }
        _ => unreachable!(),
    }
    assert!(
        matches!(&blocks[i + 1], Block::Prose(p) if p.contains("`out/shot.png`") && p.contains(&sha(PNG))),
        "{:?}",
        blocks[i + 1]
    );
}

#[cfg(feature = "pdf")]
mod pdf {
    use super::*;
    use rupu_findings_report::typst_doc;

    /// How many image XObjects a PDF holds.
    fn image_objects(pdf: &[u8]) -> usize {
        pdf.windows(14).filter(|w| *w == b"/Subtype/Image").count()
    }

    #[test]
    fn a_local_png_is_embedded_in_the_pdf() {
        let f = finding_with(vec![image(
            copied("out/shot.png", PNG),
            Some("Caption with \"quotes\" #and [brackets]"),
        )]);
        let store = FakeStore::of(&[PNG]);
        let read = |s: &str, max: u64| store.read(s, max);
        let doc = typst_doc::render_doc(&finding_blocks(&f, &HashMap::new(), Blobs::new(&read)));
        let path = format!("/evidence/{}.png", sha(PNG));
        assert_eq!(doc.files.keys().collect::<Vec<_>>(), [&path]);
        assert_eq!(&doc.files[&path][..], PNG);
        assert!(
            doc.markup.contains(&format!("#figure(image(\"{path}\"")),
            "{}",
            doc.markup
        );

        let with = render(&f, &store, Format::Pdf);
        let without = render_finding(
            &f,
            &number_map(std::slice::from_ref(&f)),
            Format::Pdf,
            Blobs::NONE,
        )
        .unwrap();
        assert!(with.starts_with(b"%PDF"));
        assert_eq!(image_objects(&with), 1, "the PDF holds the image");
        assert_eq!(image_objects(&without), 0);
    }

    #[test]
    fn an_image_typst_cannot_decode_costs_its_figure_not_the_export() {
        // PNG magic, then nothing a decoder can use.
        let mut broken = PNG[..8].to_vec();
        broken.extend_from_slice(b"definitely not IHDR");
        let f = finding_with(vec![
            image(copied("out/broken.png", &broken), Some("broken")),
            image(copied("out/fine.png", PNG), Some("fine")),
        ]);
        let store = FakeStore::of(&[&broken, PNG]);
        // HTML embeds it (the browser shows a broken image) ...
        assert!(text(&f, &store, Format::Html).contains(&data_uri("image/png", &broken)));
        // ... and the PDF still renders, with the image it can decode.
        let pdf = render(&f, &store, Format::Pdf);
        assert!(pdf.starts_with(b"%PDF"));
        assert_eq!(image_objects(&pdf), 1);
        // On its own, the broken image really does not compile.
        let alone = finding_with(vec![image(copied("out/broken.png", &broken), None)]);
        let read = |s: &str, max: u64| store.read(s, max);
        let doc =
            typst_doc::render_doc(&finding_blocks(&alone, &HashMap::new(), Blobs::new(&read)));
        assert!(rupu_findings_report::pdf::render_pdf(doc).is_err());
        assert_eq!(image_objects(&render(&alone, &store, Format::Pdf)), 0);
    }

    #[test]
    fn the_pdf_world_serves_only_the_document_s_own_images() {
        use rupu_findings_report::pdf::render_pdf;
        use rupu_findings_report::typst_doc::TypstDoc;
        let mut doc = TypstDoc::from("#image(\"/evidence/other.png\")".to_string());
        doc.files.insert(
            format!("/evidence/{}.png", sha(PNG)),
            std::sync::Arc::from(PNG),
        );
        assert!(
            render_pdf(doc.clone()).is_err(),
            "an unlisted path is refused"
        );
        doc.markup = format!("#image(\"/evidence/{}.png\")", sha(PNG));
        assert!(render_pdf(doc).is_ok());
        // Disk stays closed even with files present.
        let mut disk = TypstDoc::from("#image(\"/etc/hosts\")".to_string());
        disk.files
            .insert("/x.png".into(), std::sync::Arc::from(PNG));
        assert!(render_pdf(disk).is_err());
    }
}

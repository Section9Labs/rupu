//! Generator: parses MITRE's published CWE XML and emits a rupu-coverage
//! concerns YAML template. See `crates/rupu-coverage/build/cwe/README.md`
//! for the refresh workflow.

use std::path::PathBuf;

#[derive(Debug)]
struct Args {
    xml: PathBuf,
    view: u32,
    release: String,
    out: PathBuf,
}

fn parse_args() -> Args {
    let mut xml = None;
    let mut view = None;
    let mut release = None;
    let mut out = None;
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--xml" => xml = args.next().map(PathBuf::from),
            "--view" => view = args.next().and_then(|s| s.parse().ok()),
            "--release" => release = args.next(),
            "--out" => out = args.next().map(PathBuf::from),
            other => eprintln!("unknown flag: {other}"),
        }
    }
    Args {
        xml: xml.expect("--xml required"),
        view: view.expect("--view required (e.g. 699 or 1000)"),
        release: release.expect("--release required (e.g. 4.13)"),
        out: out.expect("--out required"),
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = parse_args();
    eprintln!(
        "generating CWE catalog: view={} release={} xml={} out={}",
        args.view,
        args.release,
        args.xml.display(),
        args.out.display(),
    );
    // Subsequent tasks (11-14) fill this in:
    // 1. Parse XML into raw weakness + view records.
    // 2. Map to Concern records (severity / applicable_globs heuristics).
    // 3. Serialize as Template YAML.
    // 4. Write .version.txt sidecar.
    eprintln!("(generator skeleton — body implemented in subsequent tasks)");
    Ok(())
}

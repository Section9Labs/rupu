//! Generator: parses MITRE's published CWE XML and emits a rupu-coverage
//! concerns YAML template.

use rupu_coverage::cwe_gen::mapping::map_view_to_concerns;
use rupu_coverage::cwe_gen::template::build_template;
use rupu_coverage::cwe_gen::xml::parse_cwe_xml;
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
        "Parsing CWE XML: view={} release={} xml={}",
        args.view,
        args.release,
        args.xml.display()
    );

    let parsed = parse_cwe_xml(&args.xml)?;
    eprintln!(
        "Parsed {} weaknesses, {} views",
        parsed.weaknesses.len(),
        parsed.views.len(),
    );

    let (namespace, view_name) = match args.view {
        699 => ("cwe-software-development", "Software Development"),
        1000 => ("cwe-research", "Research"),
        other => return Err(format!("unsupported view {other}; supported: 699, 1000").into()),
    };

    let concerns = map_view_to_concerns(&parsed, args.view, namespace)
        .ok_or_else(|| format!("view {} not found in XML", args.view))?;
    eprintln!(
        "Mapped {} concerns for view {} ({})",
        concerns.len(),
        args.view,
        view_name
    );

    let template = build_template(namespace, view_name, concerns);
    let yaml = serde_yaml::to_string(&template)?;

    if let Some(parent) = args.out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&args.out, &yaml)?;
    eprintln!("Wrote {}", args.out.display());

    // Sidecar: <out-stem>.version.txt next to the YAML.
    let mut version_path = args.out.clone();
    let stem = version_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("cwe")
        .to_string();
    version_path.set_file_name(format!("{stem}.version.txt"));
    let stamp = chrono::Utc::now().to_rfc3339();
    std::fs::write(
        &version_path,
        format!("cwe_release: {}\ngenerated_at: {}\n", args.release, stamp),
    )?;
    eprintln!("Wrote {}", version_path.display());

    Ok(())
}

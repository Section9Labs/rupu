//! `rupu skill`: manage Agent Skills in the global (`~/.rupu/skills`) and
//! project (`.rupu/skills`) layers. Thin over `rupu_skills`. Spec:
//! `docs/superpowers/specs/2026-10-12-rupu-agent-skills-design.md` §7.

use crate::output::formats::OutputFormat;
use crate::output::report::{self, CollectionOutput};
use clap::Subcommand;
use rupu_skills::git::GitFetcher;
use rupu_skills::install::{install_from, list_source, InstallOutcome, InstallSpec, Selection};
use rupu_skills::record::read_record;
use rupu_skills::tools_map::mapped_tools;
use rupu_skills::update::{update_skills, UpdateOutcome};
use rupu_skills::{parse_source, CatalogEntry, Layer, LayerDirs, SkillCatalog};
use serde::Serialize;
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Subcommand, Debug)]
pub enum Action {
    /// List installed skills in both layers.
    List {
        /// Only the project layer.
        #[arg(long)]
        project: bool,
    },
    /// Show one skill: frontmatter, files, fingerprints and source.
    /// Accepts `global:<name>` / `project:<name>`.
    Show { name: String },
    /// Check skills against the Agent Skills format; exit 1 on any failure.
    Validate {
        name: Option<String>,
        #[arg(long, conflicts_with = "name")]
        all: bool,
    },
    /// Install skills from a directory, `owner/repo`, a GitHub tree URL, or
    /// an https git URL (`#path` narrows to a folder in the repository).
    Install {
        source: String,
        /// Branch, tag or commit to install from.
        #[arg(long = "ref")]
        git_ref: Option<String>,
        /// Install this skill from a source that holds several (repeatable).
        #[arg(long = "skill")]
        skills: Vec<String>,
        /// Install every valid skill in the source.
        #[arg(long, conflicts_with = "skills")]
        all: bool,
        /// List the skills in the source; install nothing.
        #[arg(long)]
        list: bool,
        /// Install into the current project's `.rupu/skills` instead of
        /// `~/.rupu/skills`.
        #[arg(long)]
        project: bool,
        /// Replace installed skills of the same name.
        #[arg(long)]
        force: bool,
    },
    /// Re-fetch installed skills from the source they were installed from.
    Update {
        names: Vec<String>,
        /// Every installed skill with a source record.
        #[arg(long, conflicts_with = "names")]
        all: bool,
        /// The project layer instead of the global one.
        #[arg(long)]
        project: bool,
        /// Replace skills whose installed copy was edited.
        #[arg(long)]
        force: bool,
    },
}

pub fn ensure_output_format(action: &Action, format: OutputFormat) -> anyhow::Result<()> {
    let (command_name, supported) = match action {
        Action::List { .. } => ("skill list", report::TABLE_JSON_CSV),
        _ => ("skill", report::TABLE_ONLY),
    };
    crate::output::formats::ensure_supported(command_name, format, supported)
}

pub async fn handle(action: Action, format: Option<OutputFormat>) -> ExitCode {
    let result = match action {
        Action::List { project } => list(project, format).map(|()| true),
        Action::Show { name } => show(&name).map(|()| true),
        Action::Validate { name, all } => validate(name.as_deref(), all),
        Action::Install {
            source,
            git_ref,
            skills,
            all,
            list,
            project,
            force,
        } => install(
            &source,
            git_ref.as_deref(),
            skills,
            all,
            list,
            project,
            force,
        )
        .map(|()| true),
        Action::Update {
            names,
            all,
            project,
            force,
        } => update(names, all, project, force),
    };
    match result {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(e) => crate::output::diag::fail(e),
    }
}

struct Layers {
    global: PathBuf,
    project: Option<PathBuf>,
}

impl Layers {
    fn skills_dir(&self, layer: Layer) -> anyhow::Result<PathBuf> {
        match layer {
            Layer::Global => Ok(self.global.join("skills")),
            Layer::Project => self
                .project
                .as_ref()
                .map(|p| p.join("skills"))
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "not inside a rupu project (no `.rupu/` found). Run `rupu init` first, \
                         or omit --project to use ~/.rupu/skills."
                    )
                }),
        }
    }
}

fn layers() -> anyhow::Result<Layers> {
    let global = crate::paths::global_dir()?;
    let cwd = std::env::current_dir()?;
    let root = crate::paths::project_root_for(&cwd)?;
    let project = rupu_workspace::project_layer_dir(&global, root.as_deref());
    Ok(Layers { global, project })
}

fn catalog(l: &Layers) -> anyhow::Result<SkillCatalog> {
    Ok(SkillCatalog::discover(&LayerDirs {
        global: Some(l.global.join("skills")),
        project: l.project.as_ref().map(|p| p.join("skills")),
    })?)
}

fn layer_flag(project: bool) -> Layer {
    if project {
        Layer::Project
    } else {
        Layer::Global
    }
}

#[derive(Debug, Clone, Serialize)]
struct SkillRow {
    name: String,
    layer: &'static str,
    status: &'static str,
    description: String,
    source: String,
    error: String,
    path: String,
}

#[derive(Debug, Clone, Serialize)]
struct SkillsReport {
    kind: &'static str,
    version: u8,
    rows: Vec<SkillRow>,
}

struct SkillsOutput {
    report: SkillsReport,
}

impl CollectionOutput for SkillsOutput {
    type JsonReport = SkillsReport;
    type CsvRow = SkillRow;

    fn command_name(&self) -> &'static str {
        "skill list"
    }

    fn json_report(&self) -> &Self::JsonReport {
        &self.report
    }

    fn csv_rows(&self) -> &[Self::CsvRow] {
        &self.report.rows
    }

    fn render_table(&self) -> anyhow::Result<()> {
        let mut table = crate::output::tables::new_table();
        table.set_header(vec!["NAME", "LAYER", "STATUS", "SOURCE", "DESCRIPTION"]);
        for r in &self.report.rows {
            let detail = if r.status == "ok" {
                &r.description
            } else {
                &r.error
            };
            let detail: String = detail.chars().take(70).collect();
            let source = if r.source.is_empty() {
                "-".to_owned()
            } else {
                r.source.clone()
            };
            table.add_row(vec![
                r.name.clone(),
                r.layer.into(),
                r.status.into(),
                source,
                detail,
            ]);
        }
        println!("{table}");
        Ok(())
    }
}

/// The layer's `skills/` directory: the parent of the entry's skill dir.
fn skills_dir_of(e: &CatalogEntry) -> PathBuf {
    e.path.parent().map(PathBuf::from).unwrap_or_default()
}

fn row(e: &CatalogEntry) -> SkillRow {
    let (status, description, error) = match &e.skill {
        Ok(s) => ("ok", s.frontmatter.description.clone(), String::new()),
        Err(err) => ("invalid", String::new(), err.to_string()),
    };
    let source = read_record(&skills_dir_of(e), &e.dir_name)
        .ok()
        .flatten()
        .map(|r| r.source)
        .unwrap_or_default();
    SkillRow {
        name: e.dir_name.clone(),
        layer: e.layer.as_str(),
        status,
        description,
        source,
        error,
        path: e.path.display().to_string(),
    }
}

fn list(project_only: bool, format: Option<OutputFormat>) -> anyhow::Result<()> {
    let cat = catalog(&layers()?)?;
    let rows = cat
        .entries()
        .iter()
        .filter(|e| !project_only || e.layer == Layer::Project)
        .map(row)
        .collect();
    report::emit_collection(
        format,
        &SkillsOutput {
            report: SkillsReport {
                kind: "skills",
                version: 1,
                rows,
            },
        },
    )
}

/// `global:x` / `project:x` pick a layer; a bare name matches both.
fn select<'a>(cat: &'a SkillCatalog, name: &'a str) -> Vec<&'a CatalogEntry> {
    match name
        .split_once(':')
        .and_then(|(l, n)| Layer::parse(l).map(|l| (l, n)))
    {
        Some((layer, n)) => cat.get(layer, n).into_iter().collect(),
        None => cat.named(name).collect(),
    }
}

fn show(name: &str) -> anyhow::Result<()> {
    let cat = catalog(&layers()?)?;
    let found = select(&cat, name);
    anyhow::ensure!(!found.is_empty(), "no installed skill named {name:?}");
    if found.len() > 1 {
        println!(
            "{name} is installed in both layers; grant it as global:{name} or project:{name}.\n"
        );
    }
    for e in found {
        println!("{} ({})", e.dir_name, e.layer.as_str());
        println!("  path:          {}", e.path.display());
        if let Some(rec) = read_record(&skills_dir_of(e), &e.dir_name)? {
            let at = match (&rec.url, &rec.local) {
                (Some(url), _) => format!(
                    "{url}{}{} @ {}",
                    rec.git_ref
                        .as_deref()
                        .map(|r| format!(" ref {r}"))
                        .unwrap_or_default(),
                    rec.path
                        .as_deref()
                        .map(|p| format!(" #{p}"))
                        .unwrap_or_default(),
                    rec.commit
                        .as_deref()
                        .map(|c| &c[..c.len().min(12)])
                        .unwrap_or("?")
                ),
                (None, Some(dir)) => dir.display().to_string(),
                (None, None) => "?".into(),
            };
            println!("  source:        {} ({at})", rec.source);
        }
        let s = match &e.skill {
            Ok(s) => s,
            Err(err) => {
                println!("  INVALID:       {err}\n");
                continue;
            }
        };
        let fm = &s.frontmatter;
        for w in &fm.warnings {
            println!("  warning:       {w}");
        }
        println!("  description:   {}", fm.description);
        if let Some(l) = &fm.license {
            println!("  license:       {l}");
        }
        if let Some(c) = &fm.compatibility {
            println!("  compatibility: {c}");
        }
        if !fm.allowed_tools.is_empty() {
            println!(
                "  allowed-tools: {} (rupu: {}; informational, never a grant)",
                fm.allowed_tools.join(" "),
                mapped_tools(&fm.allowed_tools).join(", ")
            );
        }
        if fm.disable_model_invocation {
            println!("  model invocation: disabled (preload or operator invocation only)");
        }
        for (k, v) in &fm.metadata {
            println!("  metadata.{k}: {v}");
        }
        println!("  stamp:         {}", s.stamp.short());
        println!("  content:       {}", s.content_hash()?.short());
        println!("  files:");
        for f in s.files() {
            println!("    {f}");
        }
        println!();
    }
    Ok(())
}

fn validate(name: Option<&str>, all: bool) -> anyhow::Result<bool> {
    let cat = catalog(&layers()?)?;
    let entries: Vec<&CatalogEntry> = match name {
        Some(n) => select(&cat, n),
        None if all => cat.entries().iter().collect(),
        None => anyhow::bail!("name a skill or pass --all"),
    };
    anyhow::ensure!(
        !entries.is_empty() || all,
        "no installed skill named {:?}",
        name.unwrap_or_default()
    );
    let mut ok = true;
    for e in entries {
        match &e.skill {
            Ok(s) => {
                println!("ok       {} ({})", e.dir_name, e.layer.as_str());
                for w in &s.frontmatter.warnings {
                    println!("warn     {} ({}): {w}", e.dir_name, e.layer.as_str());
                }
            }
            Err(err) => {
                ok = false;
                println!("INVALID  {} ({}): {err}", e.dir_name, e.layer.as_str());
            }
        }
    }
    Ok(ok)
}

fn install(
    input: &str,
    git_ref: Option<&str>,
    skills: Vec<String>,
    all: bool,
    list_only: bool,
    project: bool,
    force: bool,
) -> anyhow::Result<()> {
    let home = dirs::home_dir();
    let source = parse_source(input, git_ref, home.as_deref())?;
    if list_only {
        for f in list_source(&source, &GitFetcher)? {
            let detail = match f.error {
                Some(e) => format!("INVALID: {e}"),
                None => f.description.unwrap_or_default(),
            };
            let at = if f.rel.is_empty() { ".".into() } else { f.rel };
            println!(
                "{:<28} {:<36} {}",
                f.name,
                at,
                detail.chars().take(70).collect::<String>()
            );
        }
        return Ok(());
    }
    let l = layers()?;
    let layer = layer_flag(project);
    let skills_dir = l.skills_dir(layer)?;
    let selection = if all {
        Selection::All
    } else if !skills.is_empty() {
        Selection::Named(skills)
    } else {
        Selection::Default
    };
    let report = install_from(
        &InstallSpec {
            source: &source,
            input,
            selection,
            skills_dir: &skills_dir,
            layer,
            force,
        },
        &GitFetcher,
    )?;
    for i in &report.installed {
        let verb = match i.outcome {
            InstallOutcome::Created => "installed",
            InstallOutcome::Replaced => "replaced",
        };
        println!(
            "{verb} {} ({}) at {}",
            i.name,
            layer.as_str(),
            i.path.display()
        );
    }
    for (name, why) in &report.skipped {
        eprintln!("skipped {name}: {why}");
    }
    Ok(())
}

fn update(names: Vec<String>, all: bool, project: bool, force: bool) -> anyhow::Result<bool> {
    anyhow::ensure!(
        all || !names.is_empty(),
        "name skills to update or pass --all"
    );
    let l = layers()?;
    let layer = layer_flag(project);
    let skills_dir = l.skills_dir(layer)?;
    let results = update_skills(&skills_dir, layer, &names, force, &GitFetcher);
    if results.is_empty() {
        println!(
            "no installed skills with a source record in the {} layer",
            layer.as_str()
        );
    }
    let mut ok = true;
    for (name, outcome) in results {
        let line = match outcome {
            UpdateOutcome::Updated { from, to } => format!("updated {from} → {to}"),
            UpdateOutcome::UpToDate => "up to date".into(),
            UpdateOutcome::NoRecord => {
                "no source record (installed by hand); reinstall it to track a source".into()
            }
            UpdateOutcome::LocallyModified => {
                ok = false;
                "locally modified; pass --force to replace your edits".into()
            }
            UpdateOutcome::NotInstalled => {
                ok = false;
                "not installed".into()
            }
            UpdateOutcome::Failed(e) => {
                ok = false;
                format!("failed: {e}")
            }
        };
        println!("{name}: {line}");
    }
    Ok(ok)
}

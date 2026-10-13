//! `rupu skill`: manage Agent Skills in the global (`~/.rupu/skills`) and
//! project (`.rupu/skills`) layers. Thin over `rupu_skills`. Spec:
//! `docs/superpowers/specs/2026-10-12-rupu-agent-skills-design.md` §7.

use crate::output::formats::OutputFormat;
use crate::output::printer::sanitize_terminal_text;
use crate::output::report::{self, CollectionOutput};
use clap::Subcommand;
use rupu_skills::git::GitFetcher;
use rupu_skills::install::{
    install_from, list_source, InstallError, InstallOutcome, InstallReport, InstallSpec, Selection,
};
use rupu_skills::record::{read_record, SourceRecord};
use rupu_skills::tools_map::mapped_tools;
use rupu_skills::update::{update_skills, UpdateOutcome};
use rupu_skills::{parse_source, CatalogEntry, Layer, LayerDirs, SkillCatalog};
use serde::Serialize;
use std::path::{Path, PathBuf};
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
        Err(e) => crate::output::diag::fail(sanitize_terminal_text(&e.to_string())),
    }
}

/// Skill text comes from SKILL.md files, tree walks and source records a
/// cloned repository can carry, so every line this command prints goes
/// through the terminal sanitizer.
fn say(line: &str) {
    println!("{}", sanitize_terminal_text(line));
}

fn say_err(line: &str) {
    eprintln!("{}", sanitize_terminal_text(line));
}

fn layers() -> anyhow::Result<LayerDirs> {
    let global = crate::paths::global_dir()?;
    let cwd = std::env::current_dir()?;
    let root = crate::paths::project_root_for(&cwd)?;
    let project = rupu_workspace::project_layer_dir(&global, root.as_deref());
    Ok(LayerDirs::for_layers(&global, project.as_deref()))
}

fn skills_dir(dirs: &LayerDirs, layer: Layer) -> anyhow::Result<&Path> {
    dirs.skills_dir(layer).ok_or_else(|| {
        anyhow::anyhow!(
            "not inside a rupu project (no `.rupu/` found). Run `rupu init` first, \
             or omit --project to use ~/.rupu/skills."
        )
    })
}

fn catalog(dirs: &LayerDirs) -> anyhow::Result<SkillCatalog> {
    Ok(SkillCatalog::discover(dirs)?)
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

/// `report` is what JSON emits (serde escapes control characters there);
/// `display` is the same rows sanitized for the table and CSV.
struct SkillsOutput {
    report: SkillsReport,
    display: Vec<SkillRow>,
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
        &self.display
    }

    fn render_table(&self) -> anyhow::Result<()> {
        let mut table = crate::output::tables::new_table();
        table.set_header(vec!["NAME", "LAYER", "STATUS", "SOURCE", "DESCRIPTION"]);
        for r in &self.display {
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

impl SkillRow {
    fn sanitized(&self) -> Self {
        Self {
            name: sanitize_terminal_text(&self.name),
            description: sanitize_terminal_text(&self.description),
            source: sanitize_terminal_text(&self.source),
            error: sanitize_terminal_text(&self.error),
            path: sanitize_terminal_text(&self.path),
            ..self.clone()
        }
    }
}

fn row(e: &CatalogEntry) -> SkillRow {
    let (status, description, error) = match e.skill() {
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
    let rows: Vec<SkillRow> = cat
        .entries()
        .iter()
        .filter(|e| !project_only || e.layer == Layer::Project)
        .map(row)
        .collect();
    let display = rows.iter().map(SkillRow::sanitized).collect();
    report::emit_collection(
        format,
        &SkillsOutput {
            report: SkillsReport {
                kind: "skills",
                version: 1,
                rows,
            },
            display,
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
    let Some(first) = found.first() else {
        anyhow::bail!("no installed skill named {name:?}");
    };
    let bare = &first.dir_name;
    if cat.named(bare).count() > 1 {
        say(&format!(
            "{bare} is installed in both layers; grant it as global:{bare} or project:{bare}.\n"
        ));
    }
    for e in found {
        say(&format!("{} ({})", e.dir_name, e.layer.as_str()));
        say(&format!("  path:          {}", e.path.display()));
        match read_record(&skills_dir_of(e), &e.dir_name) {
            Ok(Some(rec)) => say(&format!(
                "  source:        {} ({})",
                rec.source,
                record_at(&rec)
            )),
            Ok(None) => {}
            Err(err) => say(&format!("  source:        unreadable ({err})")),
        }
        let s = match e.skill() {
            Ok(s) => s,
            Err(err) => {
                say(&format!("  INVALID:       {err}\n"));
                continue;
            }
        };
        let fm = &s.frontmatter;
        for w in &fm.warnings {
            say(&format!("  warning:       {w}"));
        }
        say(&format!("  description:   {}", fm.description));
        if let Some(l) = &fm.license {
            say(&format!("  license:       {l}"));
        }
        if let Some(c) = &fm.compatibility {
            say(&format!("  compatibility: {c}"));
        }
        if !fm.allowed_tools.is_empty() {
            say(&format!(
                "  allowed-tools: {} (rupu: {}; informational, never a grant)",
                fm.allowed_tools.join(" "),
                mapped_tools(&fm.allowed_tools).join(", ")
            ));
        }
        if fm.disable_model_invocation {
            say("  model invocation: disabled (preload or operator invocation only)");
        }
        for (k, v) in &fm.metadata {
            say(&format!("  metadata.{k}: {v}"));
        }
        say(&format!("  stamp:         {}", s.stamp.short()));
        say(&format!("  content:       {}", s.content_hash()?.short()));
        say("  files:");
        for f in s.files() {
            say(&format!("    {f}"));
        }
        say("");
    }
    Ok(())
}

fn record_at(rec: &SourceRecord) -> String {
    match (&rec.url, &rec.local) {
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
    }
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
        match e.skill() {
            Ok(s) => {
                say(&format!("ok       {} ({})", e.dir_name, e.layer.as_str()));
                for w in &s.frontmatter.warnings {
                    say(&format!(
                        "warn     {} ({}): {w}",
                        e.dir_name,
                        e.layer.as_str()
                    ));
                }
            }
            Err(err) => {
                ok = false;
                say(&format!(
                    "INVALID  {} ({}): {err}",
                    e.dir_name,
                    e.layer.as_str()
                ));
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
            say(&format!(
                "{:<28} {:<36} {}",
                f.name,
                at,
                detail.chars().take(70).collect::<String>()
            ));
        }
        return Ok(());
    }
    let dirs = layers()?;
    let layer = layer_flag(project);
    let skills_dir = skills_dir(&dirs, layer)?;
    let selection = if all {
        Selection::All
    } else if !skills.is_empty() {
        Selection::Named(skills)
    } else {
        Selection::Default
    };
    let installed = install_from(
        &InstallSpec {
            source: &source,
            input,
            selection,
            skills_dir,
            layer,
            force,
        },
        &GitFetcher,
    );
    match installed {
        Ok(report) => {
            print_report(&report, layer);
            Ok(())
        }
        Err(InstallError::Partial { report, error }) => {
            print_report(&report, layer);
            Err(error.into())
        }
        Err(e) => Err(e.into()),
    }
}

fn print_report(report: &InstallReport, layer: Layer) {
    for i in &report.installed {
        let verb = match i.outcome {
            InstallOutcome::Created => "installed",
            InstallOutcome::Replaced => "replaced",
        };
        say(&format!(
            "{verb} {} ({}) at {}",
            i.name,
            layer.as_str(),
            i.path.display()
        ));
    }
    for name in &report.untracked {
        say(&format!(
            "note: {name} is not tracked for update (its source is outside the project); \
             reinstall it to update"
        ));
    }
    for (name, why) in &report.skipped {
        say_err(&format!("skipped {name}: {why}"));
    }
}

fn update(names: Vec<String>, all: bool, project: bool, force: bool) -> anyhow::Result<bool> {
    anyhow::ensure!(
        all || !names.is_empty(),
        "name skills to update or pass --all"
    );
    let dirs = layers()?;
    let layer = layer_flag(project);
    let skills_dir = skills_dir(&dirs, layer)?;
    let results = update_skills(skills_dir, layer, &names, force, &GitFetcher);
    if results.is_empty() {
        say(&format!(
            "no installed skills with a source record in the {} layer",
            layer.as_str()
        ));
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
        say(&format!("{name}: {line}"));
    }
    Ok(ok)
}

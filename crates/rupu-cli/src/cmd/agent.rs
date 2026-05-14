//! `rupu agent list | show <name>`.

use crate::cmd::completers::agent_names;
use crate::cmd::editor;
use crate::cmd::ui::{self, UiPrefs};
use crate::output::formats::OutputFormat;
use crate::output::report::{self, CollectionOutput, DetailOutput};
use crate::paths;
use clap::Subcommand;
use clap_complete::ArgValueCompleter;
use rupu_agent::{load_agents, AgentSpec};
use serde::Serialize;
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Subcommand, Debug)]
pub enum Action {
    /// List all available agents (global + project).
    List {
        /// Disable colored output (also honored: `NO_COLOR` env,
        /// `[ui].color = "never"` in config).
        #[arg(long)]
        no_color: bool,
    },
    /// Print an agent's frontmatter and body.
    Show {
        /// Name of the agent.
        #[arg(add = ArgValueCompleter::new(agent_names))]
        name: String,
        /// Disable colored output (also honored: `NO_COLOR` env var).
        #[arg(long)]
        no_color: bool,
        /// syntect theme name. Default: `base16-ocean.dark`.
        #[arg(long)]
        theme: Option<String>,
        /// Force pager. Default: page when stdout is a tty.
        #[arg(long, conflicts_with = "no_pager")]
        pager: bool,
        /// Disable pager.
        #[arg(long)]
        no_pager: bool,
    },
    /// Open an agent file in `$VISUAL` / `$EDITOR`. Validates the
    /// frontmatter on save (warn-only).
    Edit {
        /// Name of the agent.
        name: String,
        /// Force the project shadow (`.rupu/agents/<name>.md`) or the
        /// global file (`<global>/agents/<name>.md`). Default: prefer
        /// project if it exists, else global.
        #[arg(long, value_parser = ["global", "project"])]
        scope: Option<String>,
        /// Override the editor (e.g. `--editor "code --wait"`).
        /// Default: `$VISUAL` then `$EDITOR` then `vi`.
        #[arg(long)]
        editor: Option<String>,
    },
}

pub async fn handle(action: Action, global_format: Option<OutputFormat>) -> ExitCode {
    match action {
        Action::List { no_color } => match list(no_color, global_format).await {
            Ok(()) => ExitCode::from(0),
            Err(e) => crate::output::diag::fail(e),
        },
        Action::Show {
            name,
            no_color,
            theme,
            pager,
            no_pager,
        } => {
            let pager_flag = if pager {
                Some(true)
            } else if no_pager {
                Some(false)
            } else {
                None
            };
            match show(&name, no_color, theme.as_deref(), pager_flag, global_format).await {
                Ok(()) => ExitCode::from(0),
                Err(e) => crate::output::diag::fail(e),
            }
        }
        Action::Edit {
            name,
            scope,
            editor,
        } => match edit(&name, scope.as_deref(), editor.as_deref()).await {
            Ok(()) => ExitCode::from(0),
            Err(e) => crate::output::diag::fail(e),
        },
    }
}

pub fn ensure_output_format(action: &Action, format: OutputFormat) -> anyhow::Result<()> {
    let (command_name, supported) = match action {
        Action::List { .. } => ("agent list", report::TABLE_JSON_CSV),
        Action::Show { .. } => ("agent show", report::TABLE_JSON),
        Action::Edit { .. } => ("agent edit", report::TABLE_ONLY),
    };
    crate::output::formats::ensure_supported(command_name, format, supported)
}

#[derive(Serialize)]
struct AgentListRow {
    name: String,
    scope: String,
    description: Option<String>,
}

#[derive(Serialize)]
struct AgentListCsvRow {
    name: String,
    scope: String,
    description: String,
}

#[derive(Serialize)]
struct AgentListReport {
    kind: &'static str,
    version: u8,
    rows: Vec<AgentListRow>,
}

struct AgentListOutput {
    prefs: UiPrefs,
    report: AgentListReport,
    csv_rows: Vec<AgentListCsvRow>,
}

#[derive(Serialize)]
struct AgentShowItem {
    name: String,
    scope: String,
    path: String,
    body: String,
}

#[derive(Serialize)]
struct AgentShowReport {
    kind: &'static str,
    version: u8,
    item: AgentShowItem,
}

struct AgentShowOutput {
    prefs: UiPrefs,
    report: AgentShowReport,
}

impl CollectionOutput for AgentListOutput {
    type JsonReport = AgentListReport;
    type CsvRow = AgentListCsvRow;

    fn command_name(&self) -> &'static str {
        "agent list"
    }

    fn json_report(&self) -> &Self::JsonReport {
        &self.report
    }

    fn csv_rows(&self) -> &[Self::CsvRow] {
        &self.csv_rows
    }

    fn csv_headers(&self) -> Option<&'static [&'static str]> {
        Some(&["name", "scope", "description"])
    }

    fn render_table(&self) -> anyhow::Result<()> {
        let mut table = crate::output::tables::new_table();
        table.set_header(vec!["NAME", "SCOPE", "DESCRIPTION"]);
        for row in &self.report.rows {
            table.add_row(vec![
                comfy_table::Cell::new(&row.name),
                crate::output::tables::status_cell(&row.scope, &self.prefs),
                comfy_table::Cell::new(row.description.as_deref().unwrap_or("-")),
            ]);
        }
        println!("{table}");
        Ok(())
    }
}

impl DetailOutput for AgentShowOutput {
    type JsonReport = AgentShowReport;

    fn command_name(&self) -> &'static str {
        "agent show"
    }

    fn json_report(&self) -> &Self::JsonReport {
        &self.report
    }

    fn render_human(&self) -> anyhow::Result<()> {
        let rendered = ui::highlight_agent_file(&self.report.item.body, &self.prefs);
        ui::paginate(&rendered, &self.prefs)
    }
}

async fn list(no_color: bool, global_format: Option<OutputFormat>) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;
    let project_agents_parent = project_root.as_ref().map(|p| p.join(".rupu"));
    let agents = load_agents(&global, project_agents_parent.as_deref())?;
    let cfg = layered_config(&global, project_root.as_deref());
    let prefs = crate::cmd::ui::UiPrefs::resolve(&cfg.ui, no_color, None, None, None);

    if agents.is_empty()
        && matches!(
            global_format.unwrap_or(OutputFormat::Table),
            OutputFormat::Table
        )
    {
        println!(
            "(no agents found)\n\nDrop a `<name>.md` under `.rupu/agents/` (project) or \
             `~/.rupu/agents/` (global). See `rupu init --with-samples` for a starter set."
        );
        return Ok(());
    }
    let rows: Vec<AgentListRow> = agents
        .iter()
        .map(|agent| AgentListRow {
            name: agent.name.clone(),
            scope: scope_for(&agent.name, &global, project_agents_parent.as_deref()),
            description: agent.description.clone(),
        })
        .collect();
    let csv_rows: Vec<AgentListCsvRow> = rows
        .iter()
        .map(|row| AgentListCsvRow {
            name: row.name.clone(),
            scope: row.scope.clone(),
            description: row.description.clone().unwrap_or_default(),
        })
        .collect();
    let output = AgentListOutput {
        prefs,
        report: AgentListReport {
            kind: "agent_list",
            version: 1,
            rows,
        },
        csv_rows,
    };
    report::emit_collection(global_format, &output)
}

async fn show(
    name: &str,
    no_color: bool,
    theme: Option<&str>,
    pager_flag: Option<bool>,
    global_format: Option<OutputFormat>,
) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;
    let project_agents_parent = project_root.as_ref().map(|p| p.join(".rupu"));

    let path = locate_agent_file(name, &global, project_agents_parent.as_deref())?;
    let body = std::fs::read_to_string(&path)?;

    let cfg = layered_config(&global, project_root.as_deref());
    let prefs = UiPrefs::resolve(&cfg.ui, no_color, theme, pager_flag, None);
    let report = AgentShowReport {
        kind: "agent_show",
        version: 1,
        item: AgentShowItem {
            name: name.to_string(),
            scope: describe_scope(&path, &global).to_string(),
            path: path.display().to_string(),
            body,
        },
    };
    report::emit_detail(global_format, &AgentShowOutput { prefs, report })
}

async fn edit(
    name: &str,
    scope: Option<&str>,
    editor_override: Option<&str>,
) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;
    let project_agents_parent = project_root.as_ref().map(|p| p.join(".rupu"));

    let target = resolve_agent_path(name, scope, &global, project_agents_parent.as_deref())?;
    println!(
        "editing {} ({})",
        target.display(),
        describe_scope(&target, &global)
    );

    editor::open_for_edit(editor_override, &target)?;

    match AgentSpec::parse_file(&target) {
        Ok(_) => {
            println!("✓ {name}: frontmatter parses cleanly");
            Ok(())
        }
        Err(e) => {
            eprintln!("⚠ {name}: failed to re-parse after save:\n  {e}");
            Ok(())
        }
    }
}

fn locate_agent_file(
    name: &str,
    global: &std::path::Path,
    project_parent: Option<&std::path::Path>,
) -> anyhow::Result<std::path::PathBuf> {
    if let Some(p) = project_parent {
        let candidate = p.join("agents").join(format!("{name}.md"));
        if candidate.exists() {
            return Ok(candidate);
        }
    }
    let candidate = global.join("agents").join(format!("{name}.md"));
    if candidate.exists() {
        return Ok(candidate);
    }
    Err(anyhow::anyhow!(
        "agent `{name}` not found in project or global agents dir"
    ))
}

fn layered_config(
    global: &std::path::Path,
    project_root: Option<&std::path::Path>,
) -> rupu_config::Config {
    let global_cfg_path = global.join("config.toml");
    let project_cfg_path = project_root.map(|p| p.join(".rupu/config.toml"));
    rupu_config::layer_files(Some(&global_cfg_path), project_cfg_path.as_deref())
        .unwrap_or_default()
}

/// Pick the on-disk file to edit. With `--scope` set we honor it
/// strictly; without it we prefer the project shadow if present and
/// fall back to global.
fn resolve_agent_path(
    name: &str,
    scope: Option<&str>,
    global: &std::path::Path,
    project_parent: Option<&std::path::Path>,
) -> anyhow::Result<PathBuf> {
    let project_path = project_parent.map(|p| p.join("agents").join(format!("{name}.md")));
    let global_path = global.join("agents").join(format!("{name}.md"));

    match scope {
        Some("project") => match project_path {
            Some(p) if p.exists() => Ok(p),
            Some(p) => Err(anyhow::anyhow!(
                "agent `{name}` not found at project scope ({})",
                p.display()
            )),
            None => Err(anyhow::anyhow!(
                "no project root detected; cannot use --scope project"
            )),
        },
        Some("global") => {
            if global_path.exists() {
                Ok(global_path)
            } else {
                Err(anyhow::anyhow!(
                    "agent `{name}` not found at global scope ({})",
                    global_path.display()
                ))
            }
        }
        Some(other) => Err(anyhow::anyhow!(
            "invalid --scope `{other}` (expected `global` or `project`)"
        )),
        None => {
            if let Some(p) = project_path {
                if p.exists() {
                    return Ok(p);
                }
            }
            if global_path.exists() {
                Ok(global_path)
            } else {
                Err(anyhow::anyhow!(
                    "agent `{name}` not found in project or global agents dir"
                ))
            }
        }
    }
}

fn describe_scope(path: &std::path::Path, global: &std::path::Path) -> &'static str {
    if path.starts_with(global) {
        "global"
    } else {
        "project"
    }
}

fn scope_for(name: &str, global: &std::path::Path, project: Option<&std::path::Path>) -> String {
    if let Some(p) = project {
        if p.join("agents").join(format!("{name}.md")).exists() {
            return "project".to_string();
        }
    }
    if global.join("agents").join(format!("{name}.md")).exists() {
        "global".to_string()
    } else {
        "?".to_string()
    }
}

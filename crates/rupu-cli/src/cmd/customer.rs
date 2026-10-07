//! `rupu customer` — group projects under a customer whose config layer sits
//! between global and project. Thin: argument parsing, then
//! `rupu_workspace::CustomerStore`. Spec:
//! `docs/superpowers/specs/2026-10-06-rupu-customers-design.md`.

use crate::output::formats::OutputFormat;
use crate::output::report::{self, CollectionOutput};
use crate::paths;
use clap::Subcommand;
use comfy_table::Cell;
use rupu_workspace::{CustomerStore, MetaPatch, NewCustomer, ProjectRef};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[derive(Subcommand, Debug)]
pub enum Action {
    /// List customers.
    List {
        /// Include archived customers.
        #[arg(long)]
        archived: bool,
    },
    /// Metadata, assigned projects, and the customer's effective config.
    Show {
        slug: String,
    },
    /// Create a customer.
    Create {
        slug: String,
        #[arg(long)]
        name: String,
        #[arg(long)]
        notes: Option<String>,
        #[arg(long)]
        contact: Option<String>,
        /// `#rrggbb`.
        #[arg(long)]
        color: Option<String>,
    },
    /// Change metadata. An empty value clears `--notes`, `--contact`, `--color`.
    Set {
        slug: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        notes: Option<String>,
        #[arg(long)]
        contact: Option<String>,
        #[arg(long)]
        color: Option<String>,
    },
    /// Edit the customer's config layer in $EDITOR (validated on save).
    Edit {
        slug: String,
        /// Editor command; defaults to `[ui].editor`, then $VISUAL / $EDITOR.
        #[arg(long)]
        editor: Option<String>,
    },
    /// Hide a customer from pickers; its projects keep running.
    Archive {
        slug: String,
    },
    Unarchive {
        slug: String,
    },
    /// Delete a customer. Refused while any project is assigned.
    Delete {
        slug: String,
    },
    /// Assign a project (default: the current directory).
    Assign {
        slug: String,
        /// A project directory or workspace id (`ws_…`).
        #[arg(long)]
        project: Option<String>,
    },
    /// Remove a project's customer (default: the current directory).
    Unassign {
        #[arg(long)]
        project: Option<String>,
    },
}

pub async fn handle(action: Action, format: Option<OutputFormat>) -> ExitCode {
    match handle_inner(action, format) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => crate::output::diag::fail(e),
    }
}

fn store() -> anyhow::Result<CustomerStore> {
    Ok(CustomerStore::new(paths::global_dir()?))
}

/// `--project` as given, else the CURRENT DIRECTORY. Never the nearest
/// `.rupu/` root: `~/.rupu` is the global dir, so from a repo without its own
/// `.rupu/` that walk lands on `$HOME` and would assign the whole home
/// directory. A run looks its customer up by walking up from its own
/// directory, so the cwd (normally the repo root) is the right record.
fn project_dir(project: Option<&str>) -> anyhow::Result<ProjectArg> {
    match project {
        Some(p) if p.starts_with("ws_") => Ok(ProjectArg::Id(p.to_string())),
        Some(p) => Ok(ProjectArg::Path(PathBuf::from(p))),
        None => Ok(ProjectArg::Path(std::env::current_dir()?)),
    }
}

enum ProjectArg {
    Path(PathBuf),
    Id(String),
}

impl ProjectArg {
    fn as_ref(&self) -> ProjectRef<'_> {
        match self {
            ProjectArg::Path(p) => ProjectRef::Path(p),
            ProjectArg::Id(id) => ProjectRef::Id(id),
        }
    }
}

fn handle_inner(action: Action, format: Option<OutputFormat>) -> anyhow::Result<()> {
    let store = store()?;
    match action {
        Action::List { archived } => list(&store, archived, format),
        Action::Show { slug } => show(&store, &slug),
        Action::Create {
            slug,
            name,
            notes,
            contact,
            color,
        } => {
            let c = store.create(
                &slug,
                &NewCustomer {
                    name,
                    notes,
                    contact,
                    color,
                },
            )?;
            println!("created customer {} ({})", c.slug, c.meta.name);
            println!("config layer: {}", store.config_path(&c.slug).display());
            Ok(())
        }
        Action::Set {
            slug,
            name,
            notes,
            contact,
            color,
        } => {
            if name.is_none() && notes.is_none() && contact.is_none() && color.is_none() {
                anyhow::bail!("nothing to change — pass --name/--notes/--contact/--color");
            }
            store.update_meta(
                &slug,
                &MetaPatch {
                    name,
                    notes,
                    contact,
                    color,
                },
            )?;
            println!("updated customer {slug}");
            Ok(())
        }
        Action::Edit { slug, editor } => edit(&store, &slug, editor.as_deref()),
        Action::Archive { slug } => {
            store.set_archived(&slug, true)?;
            println!("archived customer {slug}");
            Ok(())
        }
        Action::Unarchive { slug } => {
            store.set_archived(&slug, false)?;
            println!("unarchived customer {slug}");
            Ok(())
        }
        Action::Delete { slug } => {
            store.delete(&slug)?;
            println!("deleted customer {slug}");
            Ok(())
        }
        Action::Assign { slug, project } => {
            let target = project_dir(project.as_deref())?;
            let ws = store.assign(&slug, target.as_ref())?;
            println!("assigned {} ({}) to {slug}", ws.path, ws.id);
            Ok(())
        }
        Action::Unassign { project } => {
            let target = project_dir(project.as_deref())?;
            let ws = store.unassign(target.as_ref())?;
            println!("unassigned {} ({})", ws.path, ws.id);
            Ok(())
        }
    }
}

/// Open the layer in the editor; on a parse/validation error, say why and
/// offer to re-open, so a typo never leaves a layer that fails every run.
fn edit(store: &CustomerStore, slug: &str, editor: Option<&str>) -> anyhow::Result<()> {
    store.get(slug)?;
    let path = store.config_path(slug);
    let global = paths::global_dir()?.join("config.toml");
    loop {
        crate::cmd::editor::open_for_edit(editor, &path)?;
        // What a run loads, minus the project: `Config::validate` is
        // cross-field, so the layer is checked on top of the global one.
        match rupu_config::layer_files_locked(rupu_config::LayerPaths::new(
            Some(&global),
            Some(&path),
            None,
        )) {
            Ok(_) => return Ok(()),
            Err(e) => {
                eprintln!("{} is invalid: {e}", path.display());
                if !confirm_reopen()? {
                    anyhow::bail!(
                        "left {} invalid — every run of {slug}'s projects will fail until it is fixed",
                        path.display()
                    );
                }
            }
        }
    }
}

fn confirm_reopen() -> anyhow::Result<bool> {
    use std::io::{BufRead, IsTerminal, Write};
    if !std::io::stdin().is_terminal() {
        return Ok(false);
    }
    eprint!("re-open the editor? [Y/n] ");
    std::io::stderr().flush()?;
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    Ok(!line.trim().eq_ignore_ascii_case("n"))
}

fn show(store: &CustomerStore, slug: &str) -> anyhow::Result<()> {
    let c = store.get(slug)?;
    println!("{} — {}", c.slug, c.meta.name);
    if c.meta.archived {
        println!("archived");
    }
    for (label, v) in [
        ("contact", &c.meta.contact),
        ("color", &c.meta.color),
        ("notes", &c.meta.notes),
    ] {
        if let Some(v) = v {
            println!("{label}: {v}");
        }
    }
    println!("created: {}", c.meta.created_at);

    let projects = store.projects_of(slug)?;
    println!("\nprojects ({}):", projects.len());
    for ws in &projects {
        println!("  {}  {}", ws.id, ws.path);
    }

    let global = paths::global_dir()?.join("config.toml");
    let layer = store.config_path(slug);
    println!("\nconfig layer: {}", layer.display());
    print!("{}", effective_section(&global, &layer, slug));
    Ok(())
}

/// One line of the effective-config table.
#[derive(Debug, Clone, PartialEq, Eq)]
struct EffectiveRow {
    key: String,
    value: String,
    source: String,
    locked_by: String,
}

/// Shown in place of a value that cannot be found under its provenance key.
const UNRESOLVED: &str = "<unresolved>";

/// A serde-lowercase enum (`KeySource`, `LockOwner`) as its wire string.
fn wire_name<T: Serialize>(v: &T) -> String {
    serde_json::to_value(v)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| UNRESOLVED.to_string())
}

/// Effective config for a project of this customer that sets nothing
/// itself: every key the global or customer layer sets, its value, where it
/// came from, and who locks it; plus the resolver's warnings.
fn effective_rows(global: &Path, layer: &Path) -> anyhow::Result<(Vec<EffectiveRow>, Vec<String>)> {
    let r = rupu_config::resolve(rupu_config::LayerPaths::new(
        Some(global),
        Some(layer),
        None,
    ))?;
    let flat = toml::Value::try_from(&r.config)?;
    let rows = r
        .provenance
        .iter()
        .map(|(key, prov)| EffectiveRow {
            key: key.clone(),
            value: lookup_dotted(&flat, key)
                .map(|v| v.to_string())
                .unwrap_or_else(|| UNRESOLVED.to_string()),
            source: wire_name(&prov.source),
            locked_by: prov
                .locked_by
                .as_ref()
                .map(wire_name)
                .unwrap_or_else(|| "-".into()),
        })
        .collect();
    Ok((rows, r.warnings))
}

/// The effective-config section of `rupu customer show`. A layer that does
/// not load (a malformed customer `config.toml`, say) is shown as its error
/// in place of the table, so `show` still prints the customer's metadata and
/// projects and exits 0 — it is how you find out what is wrong.
fn effective_section(global: &Path, layer: &Path, slug: &str) -> String {
    let (rows, warnings) = match effective_rows(global, layer) {
        Ok(r) => r,
        Err(e) => {
            return format!(
                "effective config unavailable — the config does not load:\n  {e:#}\n\
                 fix it with `rupu customer edit {slug}`; runs of this customer's \
                 projects fail until it loads\n"
            )
        }
    };
    let mut table = crate::output::tables::new_table();
    table.set_header(vec!["KEY", "VALUE", "SOURCE", "LOCKED BY"]);
    for r in &rows {
        table.add_row(vec![
            Cell::new(&r.key),
            Cell::new(&r.value),
            Cell::new(&r.source),
            Cell::new(&r.locked_by),
        ]);
    }
    let mut out = format!("{table}\n");
    for w in &warnings {
        out.push_str(&format!("warning: {w}\n"));
    }
    out
}

/// Walk a provenance key. Decoded with the CP write path's
/// `split_dotted_key` — the canonical decoder of the dotted-key contract —
/// never a naive `split('.')` (a model id like `GLM-5.2-FP8` is one segment).
fn lookup_dotted<'a>(v: &'a toml::Value, key: &str) -> Option<&'a toml::Value> {
    let segs = rupu_cp::config_write::split_dotted_key(key).ok()?;
    segs.iter().try_fold(v, |cur, seg| cur.get(seg.as_str()))
}

#[derive(Debug, Clone, Serialize)]
struct CustomerRow {
    slug: String,
    name: String,
    projects: usize,
    archived: bool,
    color: String,
}

#[derive(Debug, Clone, Serialize)]
struct CustomersReport {
    kind: &'static str,
    version: u8,
    rows: Vec<CustomerRow>,
}

struct CustomersOutput {
    report: CustomersReport,
}

impl CollectionOutput for CustomersOutput {
    type JsonReport = CustomersReport;
    type CsvRow = CustomerRow;

    fn command_name(&self) -> &'static str {
        "customer list"
    }

    fn json_report(&self) -> &Self::JsonReport {
        &self.report
    }

    fn csv_rows(&self) -> &[Self::CsvRow] {
        &self.report.rows
    }

    fn csv_headers(&self) -> Option<&'static [&'static str]> {
        Some(&["slug", "name", "projects", "archived", "color"])
    }

    fn render_table(&self) -> anyhow::Result<()> {
        let mut table = crate::output::tables::new_table();
        table.set_header(vec!["SLUG", "NAME", "PROJECTS", "ARCHIVED", "COLOR"]);
        for r in &self.report.rows {
            table.add_row(vec![
                Cell::new(&r.slug),
                Cell::new(&r.name),
                Cell::new(r.projects),
                Cell::new(if r.archived { "yes" } else { "" }),
                Cell::new(&r.color),
            ]);
        }
        println!("{table}");
        Ok(())
    }
}

fn list(store: &CustomerStore, archived: bool, format: Option<OutputFormat>) -> anyhow::Result<()> {
    let mut rows = Vec::new();
    for c in store.list(archived)? {
        rows.push(CustomerRow {
            projects: store.projects_of(&c.slug)?.len(),
            name: c.meta.name,
            archived: c.meta.archived,
            color: c.meta.color.unwrap_or_default(),
            slug: c.slug,
        });
    }
    report::emit_collection(
        format,
        &CustomersOutput {
            report: CustomersReport {
                kind: "customers",
                version: 1,
                rows,
            },
        },
    )
}

/// Which output formats each action supports: `list` has a collection report;
/// everything else prints plain text.
pub fn ensure_output_format(action: &Action, format: OutputFormat) -> anyhow::Result<()> {
    let (command_name, supported) = match action {
        Action::List { .. } => ("customer list", report::TABLE_JSON_CSV),
        _ => ("customer", report::TABLE_ONLY),
    };
    crate::output::formats::ensure_supported(command_name, format, supported)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, body: &str) -> std::path::PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, body).unwrap();
        p
    }

    #[test]
    fn effective_rows_show_value_source_and_lock_with_a_dotted_model_key() {
        let tmp = tempfile::tempdir().unwrap();
        let global = write(tmp.path(), "global.toml", "default_model = \"g\"\n");
        let layer = write(
            tmp.path(),
            "customer.toml",
            r#"
[pricing.oracle."GLM-5.2-FP8"]
input_per_mtok = 1.0
output_per_mtok = 2.0

[policy]
lock = ['pricing.oracle."GLM-5.2-FP8".input_per_mtok']
"#,
        );
        let (rows, _warnings) = effective_rows(&global, &layer).unwrap();
        let row = |key: &str| {
            rows.iter()
                .find(|r| r.key == key)
                .unwrap_or_else(|| panic!("no row for {key}: {rows:?}"))
                .clone()
        };

        let glm = row(r#"pricing.oracle."GLM-5.2-FP8".input_per_mtok"#);
        assert_eq!(glm.value, "1.0");
        assert_eq!(glm.source, "customer");
        assert_eq!(glm.locked_by, "customer");

        let model = row("default_model");
        assert_eq!(model.value, "\"g\"");
        assert_eq!(model.source, "global");
        assert_eq!(model.locked_by, "-");

        assert!(rows.iter().all(|r| r.value != UNRESOLVED), "{rows:?}");
    }

    #[test]
    fn effective_section_shows_a_malformed_layer_as_its_error() {
        let tmp = tempfile::tempdir().unwrap();
        let global = write(tmp.path(), "global.toml", "default_model = \"g\"\n");
        let layer = write(tmp.path(), "customer.toml", "default_provider = \n");
        let out = effective_section(&global, &layer, "acme");
        assert!(out.contains("effective config unavailable"), "{out}");
        assert!(out.contains("customer.toml"), "names the file: {out}");
        assert!(out.contains("rupu customer edit acme"), "{out}");

        let good = write(tmp.path(), "good.toml", "default_provider = \"x\"\n");
        let out = effective_section(&global, &good, "acme");
        assert!(
            out.contains("default_provider") && out.contains("customer"),
            "{out}"
        );
    }

    #[test]
    fn lookup_dotted_is_none_for_a_malformed_or_missing_key() {
        let v: toml::Value = toml::from_str("a = 1\n[b]\nc = 2\n").unwrap();
        assert_eq!(
            lookup_dotted(&v, "b.c").and_then(|v| v.as_integer()),
            Some(2)
        );
        assert!(lookup_dotted(&v, "b.missing").is_none());
        assert!(
            lookup_dotted(&v, "b.\"unterminated").is_none(),
            "decode error"
        );
    }
}

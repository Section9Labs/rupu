//! Customers: a named group of projects with its own config layer.
//!
//! Spec: `docs/superpowers/specs/2026-10-06-rupu-customers-design.md`.
//! Layout under `RUPU_HOME`:
//!
//! ```text
//! customers/<slug>/customer.toml   metadata (CustomerMeta)
//! customers/<slug>/config.toml     the customer's config layer
//! workspaces/<ws_id>.customer      assignment sidecar: the slug
//! ```
//!
//! The assignment is a sidecar, not a `Workspace` field, because
//! [`crate::upsert`] rewrites the whole workspace record, unlocked, on every
//! run: a field would let a run that started while you assigned write back
//! its stale copy. `upsert` never touches the sidecar.

use crate::record::Workspace;
use crate::store::{find_by_path, register, StoreError, WorkspaceStore};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use thiserror::Error;
use tracing::warn;

#[derive(Debug, Error)]
pub enum CustomerError {
    #[error(
        "invalid customer slug `{0}`: use 1-63 characters of a-z, 0-9 and `-`, \
         starting with a letter or digit"
    )]
    InvalidSlug(String),
    #[error("invalid color `{0}`: use #rrggbb")]
    InvalidColor(String),
    #[error("customer name must not be empty")]
    EmptyName,
    #[error("customer `{0}` already exists")]
    Exists(String),
    #[error("no customer `{0}`")]
    NotFound(String),
    #[error("customer `{0}` is archived; unarchive it before assigning projects")]
    Archived(String),
    #[error("customer `{slug}` still has assigned projects: {}", .projects.join(", "))]
    HasProjects { slug: String, projects: Vec<String> },
    #[error(
        "project {project} is assigned to customer `{slug}`, which does not exist \
         (recreate the customer, or remove {sidecar})"
    )]
    Dangling {
        project: String,
        slug: String,
        sidecar: String,
    },
    #[error("no rupu project is registered as {0}")]
    NoProject(String),
    #[error("invalid project id `{0}`: use letters, digits, `-` and `_`")]
    InvalidWsId(String),
    #[error(
        "customer assignment {sidecar} cannot be resolved: {reason} \
         (repair the workspace record, or remove the sidecar)"
    )]
    UnresolvableAssignment { sidecar: String, reason: String },
    #[error(
        "project {project} has two workspace records assigned to different customers: \
         `{first_slug}` ({first_sidecar}) and `{second_slug}` ({second_sidecar}); \
         remove one of the sidecars"
    )]
    ConflictingAssignments {
        project: String,
        first_slug: String,
        first_sidecar: String,
        second_slug: String,
        second_sidecar: String,
    },
    #[error("io {action}: {source}")]
    Io {
        action: String,
        #[source]
        source: std::io::Error,
    },
    #[error("parse {path}: {source}")]
    Parse {
        path: String,
        #[source]
        source: toml::de::Error,
    },
    #[error("serialize: {0}")]
    Ser(#[from] toml::ser::Error),
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// `customers/<slug>/customer.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CustomerMeta {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contact: Option<String>,
    /// `#rrggbb`. `None` ⇒ consumers derive a tint from the slug.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default)]
    pub archived: bool,
    /// RFC 3339.
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Customer {
    pub slug: String,
    pub meta: CustomerMeta,
}

/// Input to [`CustomerStore::create`].
#[derive(Debug, Clone, Default)]
pub struct NewCustomer {
    pub name: String,
    pub notes: Option<String>,
    pub contact: Option<String>,
    pub color: Option<String>,
}

/// Input to [`CustomerStore::update_meta`]. `None` leaves a field alone;
/// `Some("")` clears an optional field (`notes`, `contact`, `color`).
#[derive(Debug, Clone, Default)]
pub struct MetaPatch {
    pub name: Option<String>,
    pub notes: Option<String>,
    pub contact: Option<String>,
    pub color: Option<String>,
}

/// A project named by its directory or by its workspace id.
#[derive(Debug, Clone, Copy)]
pub enum ProjectRef<'a> {
    Path(&'a Path),
    Id(&'a str),
}

pub fn validate_slug(slug: &str) -> Result<(), CustomerError> {
    let mut chars = slug.chars();
    let ok = match chars.next() {
        Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit() => {
            slug.len() <= 63
                && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        }
        _ => false,
    };
    if ok {
        Ok(())
    } else {
        Err(CustomerError::InvalidSlug(slug.to_string()))
    }
}

/// A workspace id's shape: non-empty letters, digits, `-` and `_` (a ULID
/// in practice). Checked before an id is joined onto a path, so an id from
/// a caller (`../../etc`) can never name a file outside the store.
pub fn validate_ws_id(id: &str) -> Result<(), CustomerError> {
    let valid = !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if valid {
        Ok(())
    } else {
        Err(CustomerError::InvalidWsId(id.to_string()))
    }
}

fn validate_color(color: &str) -> Result<(), CustomerError> {
    let hex = color.strip_prefix('#').unwrap_or("");
    if hex.len() == 6 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(CustomerError::InvalidColor(color.to_string()))
    }
}

fn validate_name(name: &str) -> Result<String, CustomerError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        Err(CustomerError::EmptyName)
    } else {
        Ok(trimmed.to_string())
    }
}

/// `Some("")` ⇒ cleared, `Some(v)` ⇒ set, `None` ⇒ unchanged.
fn patch_opt(slot: &mut Option<String>, value: &Option<String>) {
    if let Some(v) = value {
        *slot = if v.is_empty() { None } else { Some(v.clone()) };
    }
}

const LAYER_HEADER: &str = "# Customer config layer. Same schema as ~/.rupu/config.toml.\n\
# Precedence: global lock > customer lock ([policy].lock here) > project > customer > global.\n";

#[derive(Debug, Clone)]
pub struct CustomerStore {
    pub home: PathBuf,
}

impl CustomerStore {
    pub fn new(home: impl Into<PathBuf>) -> Self {
        Self { home: home.into() }
    }

    fn workspaces(&self) -> WorkspaceStore {
        WorkspaceStore {
            root: self.home.join("workspaces"),
        }
    }

    fn customers_dir(&self) -> PathBuf {
        self.home.join("customers")
    }

    fn dir(&self, slug: &str) -> PathBuf {
        self.customers_dir().join(slug)
    }

    fn meta_path(&self, slug: &str) -> PathBuf {
        self.dir(slug).join("customer.toml")
    }

    /// `customers/<slug>/config.toml` — the customer's config layer.
    pub fn config_path(&self, slug: &str) -> PathBuf {
        self.dir(slug).join("config.toml")
    }

    fn exists(&self, slug: &str) -> bool {
        self.meta_path(slug).is_file()
    }

    fn read_meta(&self, slug: &str) -> Result<CustomerMeta, CustomerError> {
        let path = self.meta_path(slug);
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(CustomerError::NotFound(slug.to_string()))
            }
            Err(e) => {
                return Err(CustomerError::Io {
                    action: format!("read {}", path.display()),
                    source: e,
                })
            }
        };
        toml::from_str(&text).map_err(|e| CustomerError::Parse {
            path: path.display().to_string(),
            source: e,
        })
    }

    fn write_meta(&self, slug: &str, meta: &CustomerMeta) -> Result<(), CustomerError> {
        write_atomic(&self.meta_path(slug), &toml::to_string(meta)?)
    }

    pub fn list(&self, include_archived: bool) -> Result<Vec<Customer>, CustomerError> {
        let dir = self.customers_dir();
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
            Err(e) => {
                return Err(CustomerError::Io {
                    action: format!("read_dir {}", dir.display()),
                    source: e,
                })
            }
        };
        let mut out = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| CustomerError::Io {
                action: "read_dir entry".into(),
                source: e,
            })?;
            let Some(slug) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            if validate_slug(&slug).is_err() || !self.exists(&slug) {
                continue;
            }
            match self.read_meta(&slug) {
                Ok(meta) if include_archived || !meta.archived => {
                    out.push(Customer { slug, meta });
                }
                Ok(_) => {}
                Err(e) => warn!(slug = %slug, error = %e, "skipping unreadable customer"),
            }
        }
        out.sort_by(|a, b| a.slug.cmp(&b.slug));
        Ok(out)
    }

    pub fn get(&self, slug: &str) -> Result<Customer, CustomerError> {
        validate_slug(slug)?;
        Ok(Customer {
            slug: slug.to_string(),
            meta: self.read_meta(slug)?,
        })
    }

    pub fn create(&self, slug: &str, new: &NewCustomer) -> Result<Customer, CustomerError> {
        validate_slug(slug)?;
        let name = validate_name(&new.name)?;
        let color = new.color.clone().filter(|s| !s.is_empty());
        if let Some(c) = &color {
            validate_color(c)?;
        }
        if self.exists(slug) {
            return Err(CustomerError::Exists(slug.to_string()));
        }
        let meta = CustomerMeta {
            name,
            notes: new.notes.clone().filter(|s| !s.is_empty()),
            contact: new.contact.clone().filter(|s| !s.is_empty()),
            color,
            archived: false,
            created_at: Utc::now().to_rfc3339(),
        };
        let dir = self.dir(slug);
        std::fs::create_dir_all(&dir).map_err(|e| CustomerError::Io {
            action: format!("create_dir_all {}", dir.display()),
            source: e,
        })?;
        if !self.config_path(slug).exists() {
            write_atomic(&self.config_path(slug), LAYER_HEADER)?;
        }
        self.write_meta(slug, &meta)?;
        Ok(Customer {
            slug: slug.to_string(),
            meta,
        })
    }

    pub fn update_meta(&self, slug: &str, patch: &MetaPatch) -> Result<Customer, CustomerError> {
        let mut c = self.get(slug)?;
        if let Some(n) = &patch.name {
            c.meta.name = validate_name(n)?;
        }
        if let Some(col) = patch.color.as_deref().filter(|s| !s.is_empty()) {
            validate_color(col)?;
        }
        patch_opt(&mut c.meta.notes, &patch.notes);
        patch_opt(&mut c.meta.contact, &patch.contact);
        patch_opt(&mut c.meta.color, &patch.color);
        self.write_meta(slug, &c.meta)?;
        Ok(c)
    }

    pub fn set_archived(&self, slug: &str, archived: bool) -> Result<Customer, CustomerError> {
        let mut c = self.get(slug)?;
        c.meta.archived = archived;
        self.write_meta(slug, &c.meta)?;
        Ok(c)
    }

    pub fn delete(&self, slug: &str) -> Result<(), CustomerError> {
        self.get(slug)?;
        let projects = self.projects_of(slug)?;
        if !projects.is_empty() {
            return Err(CustomerError::HasProjects {
                slug: slug.to_string(),
                projects: projects.into_iter().map(|w| w.path).collect(),
            });
        }
        let dir = self.dir(slug);
        std::fs::remove_dir_all(&dir).map_err(|e| CustomerError::Io {
            action: format!("remove_dir_all {}", dir.display()),
            source: e,
        })
    }

    fn resolve_project(
        &self,
        project: ProjectRef<'_>,
        register_missing: bool,
    ) -> Result<Workspace, CustomerError> {
        let store = self.workspaces();
        match project {
            ProjectRef::Id(id) => {
                validate_ws_id(id)?;
                store
                    .load(id)?
                    .ok_or_else(|| CustomerError::NoProject(id.to_string()))
            }
            ProjectRef::Path(p) if register_missing => Ok(register(&store, p)?),
            ProjectRef::Path(p) => find_by_path(&store, p)?
                .ok_or_else(|| CustomerError::NoProject(p.display().to_string())),
        }
    }

    pub fn assign(&self, slug: &str, project: ProjectRef<'_>) -> Result<Workspace, CustomerError> {
        let c = self.get(slug)?;
        if c.meta.archived {
            return Err(CustomerError::Archived(slug.to_string()));
        }
        let ws = self.resolve_project(project, true)?;
        write_atomic(
            &self.workspaces().customer_sidecar_path(&ws.id),
            &format!("{slug}\n"),
        )?;
        Ok(ws)
    }

    /// Idempotent: unassigning a project with no customer succeeds.
    pub fn unassign(&self, project: ProjectRef<'_>) -> Result<Workspace, CustomerError> {
        let ws = self.resolve_project(project, false)?;
        let sidecar = self.workspaces().customer_sidecar_path(&ws.id);
        match std::fs::remove_file(&sidecar) {
            Ok(()) => Ok(ws),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(ws),
            Err(e) => Err(CustomerError::Io {
                action: format!("remove {}", sidecar.display()),
                source: e,
            }),
        }
    }

    /// The slug in `ws_id`'s sidecar, as written — not checked against the
    /// customer directory (see [`Self::customer_config_for_dir`] for that).
    pub fn customer_of(&self, ws_id: &str) -> Result<Option<String>, CustomerError> {
        validate_ws_id(ws_id)?;
        read_sidecar(&self.workspaces().customer_sidecar_path(ws_id))
    }

    /// Every assignment sidecar (`workspaces/<id>.customer`) with a
    /// non-empty slug. No sidecars (the common case) costs one `read_dir`.
    fn assignments(&self) -> Result<Vec<Assignment>, CustomerError> {
        let root = self.workspaces().root;
        let entries = match std::fs::read_dir(&root) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
            Err(e) => {
                return Err(CustomerError::Io {
                    action: format!("read_dir {}", root.display()),
                    source: e,
                })
            }
        };
        let mut out = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| CustomerError::Io {
                action: "read_dir entry".into(),
                source: e,
            })?;
            let sidecar = entry.path();
            if sidecar.extension().and_then(|s| s.to_str()) != Some("customer") {
                continue;
            }
            let id = sidecar
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_string();
            let Some(slug) = read_sidecar(&sidecar)? else {
                continue;
            };
            out.push(Assignment { sidecar, id, slug });
        }
        // Deterministic order, so a conflict error names the same pair
        // every time.
        out.sort_by(|a, b| a.sidecar.cmp(&b.sidecar));
        Ok(out)
    }

    /// The workspace record a sidecar assigns. A sidecar whose record is
    /// missing, unreadable or corrupt is an error, never skipped: skipping
    /// it would run that project on global config.
    fn assigned_record(&self, a: &Assignment) -> Result<Workspace, CustomerError> {
        let unresolvable = |reason: String| CustomerError::UnresolvableAssignment {
            sidecar: a.sidecar.display().to_string(),
            reason,
        };
        validate_ws_id(&a.id).map_err(|e| unresolvable(e.to_string()))?;
        match self.workspaces().load(&a.id) {
            Ok(Some(ws)) => Ok(ws),
            Ok(None) => Err(unresolvable(format!(
                "workspace record {}.toml is missing",
                a.id
            ))),
            Err(e) => Err(unresolvable(e.to_string())),
        }
    }

    /// The projects assigned to `slug`. Fails when one of its sidecars
    /// names a record that cannot be read, so [`Self::delete`] never
    /// removes a customer a project still points at.
    pub fn projects_of(&self, slug: &str) -> Result<Vec<Workspace>, CustomerError> {
        let mut out = Vec::new();
        for a in self.assignments()? {
            if a.slug == slug {
                out.push(self.assigned_record(&a)?);
            }
        }
        out.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(out)
    }

    /// The customer that owns runs in `dir`: the nearest ancestor of `dir`
    /// (inclusive) whose workspace record has an assignment. Workspace
    /// records are keyed by the directory rupu ran in, so a run from a
    /// subdirectory has its own record; walking up lets it inherit its
    /// project's customer. Errors with [`CustomerError::Dangling`] when the
    /// nearest assignment names a customer that no longer exists — callers
    /// on launch paths must fail the run, never fall back to global config.
    /// A `dir` that does not exist (a deleted worktree, say) is looked up
    /// from its nearest existing ancestor.
    ///
    /// Driven by the assignment sidecars, and fail-closed: a sidecar whose
    /// workspace record cannot be read is an error
    /// ([`CustomerError::UnresolvableAssignment`]), as are two records for
    /// the same directory assigned to different customers
    /// ([`CustomerError::ConflictingAssignments`]). A record whose directory
    /// no longer exists is skipped — nothing can run there.
    pub fn customer_for_dir(&self, dir: &Path) -> Result<Option<String>, CustomerError> {
        let assignments = self.assignments()?;
        if assignments.is_empty() {
            return Ok(None);
        }
        let canonical = canonicalize_nearest(dir)?;
        let mut by_path: BTreeMap<PathBuf, Vec<&Assignment>> = BTreeMap::new();
        for a in &assignments {
            let ws = self.assigned_record(a)?;
            match Path::new(&ws.path).canonicalize() {
                Ok(p) => by_path.entry(p).or_default().push(a),
                Err(e) => warn!(
                    sidecar = %a.sidecar.display(),
                    path = %ws.path,
                    error = %e,
                    "skipping assignment whose project directory is gone"
                ),
            }
        }
        for ancestor in canonical.ancestors() {
            let Some(found) = by_path.get(ancestor) else {
                continue;
            };
            let first = found[0];
            if let Some(other) = found.iter().find(|a| a.slug != first.slug) {
                return Err(CustomerError::ConflictingAssignments {
                    project: ancestor.display().to_string(),
                    first_slug: first.slug.clone(),
                    first_sidecar: first.sidecar.display().to_string(),
                    second_slug: other.slug.clone(),
                    second_sidecar: other.sidecar.display().to_string(),
                });
            }
            if validate_slug(&first.slug).is_err() || !self.exists(&first.slug) {
                return Err(CustomerError::Dangling {
                    project: ancestor.display().to_string(),
                    slug: first.slug.clone(),
                    sidecar: first.sidecar.display().to_string(),
                });
            }
            return Ok(Some(first.slug.clone()));
        }
        Ok(None)
    }

    /// [`Self::customer_for_dir`], as the path of that customer's layer.
    pub fn customer_config_for_dir(&self, dir: &Path) -> Result<Option<PathBuf>, CustomerError> {
        Ok(self
            .customer_for_dir(dir)?
            .map(|slug| self.config_path(&slug)))
    }
}

/// One `workspaces/<id>.customer` sidecar.
#[derive(Debug)]
struct Assignment {
    sidecar: PathBuf,
    id: String,
    slug: String,
}

/// A sidecar's slug; `None` when the file is absent or blank.
fn read_sidecar(path: &Path) -> Result<Option<String>, CustomerError> {
    match std::fs::read_to_string(path) {
        Ok(s) => Ok(Some(s.trim().to_string()).filter(|s| !s.is_empty())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(CustomerError::Io {
            action: format!("read {}", path.display()),
            source: e,
        }),
    }
}

/// `dir` canonicalized, or — when it does not exist — its nearest existing
/// ancestor canonicalized. Errors only when no ancestor resolves.
fn canonicalize_nearest(dir: &Path) -> Result<PathBuf, CustomerError> {
    let first_err = match dir.canonicalize() {
        Ok(p) => return Ok(p),
        Err(e) => e,
    };
    dir.ancestors()
        .skip(1)
        .filter(|a| !a.as_os_str().is_empty())
        .find_map(|a| a.canonicalize().ok())
        .ok_or_else(|| CustomerError::Io {
            action: format!("canonicalize {}", dir.display()),
            source: first_err,
        })
}

/// A uniquely named temp file in the target's directory + rename, so readers
/// never see a partial file, concurrent writers never share (and tear) one
/// temp file, and a failed write or rename leaves no temp file behind
/// (`NamedTempFile` removes itself on drop).
fn write_atomic(path: &Path, body: &str) -> Result<(), CustomerError> {
    use std::io::Write;
    let parent = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    std::fs::create_dir_all(parent).map_err(|e| CustomerError::Io {
        action: format!("create_dir_all {}", parent.display()),
        source: e,
    })?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent).map_err(|e| CustomerError::Io {
        action: format!("create temp file in {}", parent.display()),
        source: e,
    })?;
    tmp.write_all(body.as_bytes())
        .and_then(|()| tmp.as_file().sync_all())
        .map_err(|e| CustomerError::Io {
            action: format!("write temp file for {}", path.display()),
            source: e,
        })?;
    tmp.persist(path).map_err(|e| CustomerError::Io {
        action: format!("rename temp file -> {}", path.display()),
        source: e.error,
    })?;
    Ok(())
}

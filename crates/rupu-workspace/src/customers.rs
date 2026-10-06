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
        if let Some(c) = &new.color {
            validate_color(c)?;
        }
        if self.exists(slug) {
            return Err(CustomerError::Exists(slug.to_string()));
        }
        let meta = CustomerMeta {
            name,
            notes: new.notes.clone().filter(|s| !s.is_empty()),
            contact: new.contact.clone().filter(|s| !s.is_empty()),
            color: new.color.clone(),
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
            ProjectRef::Id(id) => store
                .load(id)?
                .ok_or_else(|| CustomerError::NoProject(id.to_string())),
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
        let path = self.workspaces().customer_sidecar_path(ws_id);
        match std::fs::read_to_string(&path) {
            Ok(s) => Ok(Some(s.trim().to_string()).filter(|s| !s.is_empty())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(CustomerError::Io {
                action: format!("read {}", path.display()),
                source: e,
            }),
        }
    }

    pub fn projects_of(&self, slug: &str) -> Result<Vec<Workspace>, CustomerError> {
        let mut out = Vec::new();
        for ws in self.workspaces().list()? {
            if self.customer_of(&ws.id)?.as_deref() == Some(slug) {
                out.push(ws);
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
    pub fn customer_for_dir(&self, dir: &Path) -> Result<Option<String>, CustomerError> {
        let canonical = match dir.canonicalize() {
            Ok(p) => p,
            Err(e) => {
                return Err(CustomerError::Io {
                    action: format!("canonicalize {}", dir.display()),
                    source: e,
                })
            }
        };
        // One pass over the records, keyed by canonical path.
        let mut by_path: BTreeMap<PathBuf, String> = BTreeMap::new();
        for ws in self.workspaces().list()? {
            if let Ok(p) = Path::new(&ws.path).canonicalize() {
                by_path.insert(p, ws.id);
            }
        }
        for ancestor in canonical.ancestors() {
            let Some(id) = by_path.get(ancestor) else {
                continue;
            };
            let Some(slug) = self.customer_of(id)? else {
                continue;
            };
            if validate_slug(&slug).is_err() || !self.exists(&slug) {
                return Err(CustomerError::Dangling {
                    project: ancestor.display().to_string(),
                    slug,
                    sidecar: self
                        .workspaces()
                        .customer_sidecar_path(id)
                        .display()
                        .to_string(),
                });
            }
            return Ok(Some(slug));
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

/// Temp file + rename, so readers never see a partial file.
fn write_atomic(path: &Path, body: &str) -> Result<(), CustomerError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| CustomerError::Io {
            action: format!("create_dir_all {}", parent.display()),
            source: e,
        })?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, body).map_err(|e| CustomerError::Io {
        action: format!("write {}", tmp.display()),
        source: e,
    })?;
    std::fs::rename(&tmp, path).map_err(|e| CustomerError::Io {
        action: format!("rename {} -> {}", tmp.display(), path.display()),
        source: e,
    })
}

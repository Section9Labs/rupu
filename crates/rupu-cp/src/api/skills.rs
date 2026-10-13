//! Installed Agent Skills, read-only (spec
//! `docs/superpowers/specs/2026-10-12-rupu-agent-skills-design.md` §8).
//!
//! `GET /api/skills` lists the global layer; `?ws_id=` adds that project's
//! layer. Each row loads its skill to show its status, as `rupu skill list`
//! does; a layer that can't be read is skipped and named in the response's
//! `warnings`. `GET /api/skills/:layer/:name` shows one skill, as `rupu
//! skill show` does.

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::{
    routing::{get, post, put},
    Json, Router,
};
use rupu_skills::record::RecordError;
use rupu_skills::ship::{self, ShipError, ShippedSkillRef, MAX_SHIPPED_SKILL_BYTES};
use rupu_skills::tools_map::mapped_tools;
use rupu_skills::{CatalogEntry, Layer, LayerDirs, SkillCatalog};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/skills", get(list_skills))
        .route("/api/skills/:layer/:name", get(get_skill))
        // Shipped skill content (placed units). The static `bundle` segment
        // wins over `:layer`, so the routes above keep serving.
        .route("/api/skills/bundle/missing", post(bundle_missing))
        .route(
            "/api/skills/bundle/:hash",
            put(bundle_put).layer(DefaultBodyLimit::max(
                MAX_SHIPPED_SKILL_BYTES as usize + (64 << 10),
            )),
        )
}

#[derive(Deserialize)]
struct MissingBody {
    skills: Vec<ShippedSkillRef>,
}

/// A `ShipError` caused by the request is a 400; a disk failure is a 500.
fn ship_err(e: ShipError) -> ApiError {
    match e {
        ShipError::Io { .. } => ApiError::internal(e.to_string()),
        other => ApiError::bad_request(other.to_string()),
    }
}

async fn bundle_missing(
    State(s): State<AppState>,
    Json(body): Json<MissingBody>,
) -> ApiResult<Json<serde_json::Value>> {
    for r in &body.skills {
        r.validate().map_err(ship_err)?;
    }
    let cache = ship::cache_dir(&s.global_dir);
    let missing = tokio::task::spawn_blocking(move || ship::missing(&cache, &body.skills))
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(serde_json::json!({ "missing": missing })))
}

#[derive(Deserialize)]
struct PutQuery {
    layer: String,
    name: String,
}

async fn bundle_put(
    State(s): State<AppState>,
    Path(hash): Path<String>,
    Query(q): Query<PutQuery>,
    body: Bytes,
) -> ApiResult<Json<serde_json::Value>> {
    let layer = Layer::parse(&q.layer)
        .ok_or_else(|| ApiError::bad_request(format!("unknown layer {:?}", q.layer)))?;
    let r = ShippedSkillRef {
        layer,
        name: q.name,
        content_hash: hash.clone(),
    };
    let cache = ship::cache_dir(&s.global_dir);
    tokio::task::spawn_blocking(move || ship::store(&cache, &r, &body))
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?
        .map_err(ship_err)?;
    Ok(Json(serde_json::json!({ "stored": hash })))
}

#[derive(Debug, Serialize)]
pub struct SkillRow {
    pub name: String,
    pub layer: &'static str,
    /// `ok` or `invalid`.
    pub status: &'static str,
    pub description: Option<String>,
    pub error: Option<String>,
    pub warnings: Vec<String>,
    pub stamp: Option<String>,
    /// The `source` of the skill's install record, when it has one. A
    /// record that can't be read is a `warnings` entry.
    pub source: Option<String>,
    /// The other layer has a skill of the same name.
    pub both_layers: bool,
}

#[derive(Debug, Serialize)]
pub struct SkillsResponse {
    pub skills: Vec<SkillRow>,
    /// Each layer that couldn't be read, and why: its skills are missing
    /// from `skills`.
    pub warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct SkillDetail {
    #[serde(flatten)]
    pub row: SkillRow,
    pub path: String,
    pub license: Option<String>,
    pub compatibility: Option<String>,
    pub allowed_tools: Vec<String>,
    pub mapped_tools: Vec<&'static str>,
    pub disable_model_invocation: bool,
    pub metadata: BTreeMap<String, String>,
    /// The skill's files other than `SKILL.md`, relative to its root.
    pub files: Vec<String>,
    pub content_hash: Option<String>,
    /// `SKILL.md` without its frontmatter.
    pub body: Option<String>,
    pub source_record: Option<serde_json::Value>,
    /// Why the install record could not be read, when it couldn't.
    pub source_error: Option<String>,
}

#[derive(Deserialize)]
struct SkillsQuery {
    ws_id: Option<String>,
}

/// The layers a request reads: global, plus the project's when `ws_id`
/// names a registered workspace. Reads the disk.
fn layers(s: &AppState, ws_id: Option<&str>) -> ApiResult<LayerDirs> {
    let project = match ws_id {
        None => None,
        Some(id) => {
            let w = crate::api::code::load_workspace(s, id)?;
            rupu_workspace::project_layer_dir(&s.global_dir, Some(std::path::Path::new(&w.path)))
        }
    };
    Ok(LayerDirs::for_layers(&s.global_dir, project.as_deref()))
}

/// The skills of every layer that can be read, and a warning naming each
/// that can't.
fn discover(dirs: &LayerDirs) -> (SkillCatalog, Vec<(Layer, String)>) {
    let (cat, failed) = SkillCatalog::discover_readable(dirs);
    let failed = failed
        .into_iter()
        .map(|(layer, e)| (layer, format!("{} layer: {e}", layer.as_str())))
        .collect();
    (cat, failed)
}

fn row(e: &CatalogEntry, cat: &SkillCatalog) -> SkillRow {
    let (status, description, error, mut warnings, stamp) = match e.skill() {
        Ok(s) => (
            "ok",
            Some(s.frontmatter.description.clone()),
            None,
            s.frontmatter.warnings.clone(),
            Some(s.stamp.short()),
        ),
        Err(err) => ("invalid", None, Some(err.to_string()), Vec::new(), None),
    };
    let source = match e.record() {
        Ok(rec) => rec.map(|r| r.source),
        // The skill's own error already says why.
        Err(RecordError::LinkedRoot) => None,
        Err(err) => {
            warnings.push(format!("install record: {err}"));
            None
        }
    };
    SkillRow {
        name: e.dir_name.clone(),
        layer: e.layer.as_str(),
        status,
        description,
        error,
        warnings,
        stamp,
        source,
        both_layers: cat.named(&e.dir_name).count() > 1,
    }
}

fn detail(e: &CatalogEntry, cat: &SkillCatalog) -> SkillDetail {
    let (source_record, source_error) = match e.record() {
        Ok(rec) => (
            rec.map(|r| serde_json::to_value(r).expect("a source record serializes")),
            None,
        ),
        Err(err) => (None, Some(err.to_string())),
    };
    let mut out = SkillDetail {
        row: row(e, cat),
        path: e.path.display().to_string(),
        license: None,
        compatibility: None,
        allowed_tools: Vec::new(),
        mapped_tools: Vec::new(),
        disable_model_invocation: false,
        metadata: BTreeMap::new(),
        files: Vec::new(),
        content_hash: None,
        body: None,
        source_record,
        source_error,
    };
    let Ok(s) = e.skill() else {
        return out;
    };
    match s.content_hash() {
        Ok(h) => out.content_hash = Some(h.short()),
        Err(err) => out.row.warnings.push(format!("content hash: {err}")),
    }
    let fm = &s.frontmatter;
    out.license = fm.license.clone();
    out.compatibility = fm.compatibility.clone();
    out.allowed_tools = fm.allowed_tools.clone();
    out.mapped_tools = mapped_tools(&fm.allowed_tools);
    out.disable_model_invocation = fm.disable_model_invocation;
    out.metadata = fm.metadata.clone();
    out.files = s
        .files()
        .into_iter()
        .filter(|f| *f != "SKILL.md")
        .map(str::to_owned)
        .collect();
    out.body = Some(s.body.clone());
    out
}

async fn list_skills(
    State(s): State<AppState>,
    Query(q): Query<SkillsQuery>,
) -> ApiResult<Json<SkillsResponse>> {
    tokio::task::spawn_blocking(move || {
        let (cat, failed) = discover(&layers(&s, q.ws_id.as_deref())?);
        Ok(Json(SkillsResponse {
            skills: cat.entries().iter().map(|e| row(e, &cat)).collect(),
            warnings: failed.into_iter().map(|(_, w)| w).collect(),
        }))
    })
    .await
    .map_err(|e| ApiError::internal(e.to_string()))?
}

async fn get_skill(
    State(s): State<AppState>,
    Path((layer, name)): Path<(String, String)>,
    Query(q): Query<SkillsQuery>,
) -> ApiResult<Json<SkillDetail>> {
    let layer = Layer::parse(&layer).ok_or_else(|| {
        ApiError::bad_request(format!(
            "unknown layer {layer:?}; expected global or project"
        ))
    })?;
    if layer == Layer::Project && q.ws_id.is_none() {
        return Err(ApiError::bad_request(
            "a project skill needs ?ws_id= naming its project",
        ));
    }
    tokio::task::spawn_blocking(move || {
        let (cat, failed) = discover(&layers(&s, q.ws_id.as_deref())?);
        if let Some((_, w)) = failed.into_iter().find(|(l, _)| *l == layer) {
            return Err(ApiError::internal(w));
        }
        cat.get(layer, &name)
            .map(|e| Json(detail(e, &cat)))
            .ok_or_else(|| {
                ApiError::not_found(format!("no installed skill {}:{name}", layer.as_str()))
            })
    })
    .await
    .map_err(|e| ApiError::internal(e.to_string()))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use http::{Request, StatusCode};
    use http_body_util::BodyExt as _;
    use std::path::Path as FsPath;
    use tower::ServiceExt as _;

    fn test_state(tmp: &tempfile::TempDir) -> AppState {
        AppState::new(
            tmp.path().to_path_buf(),
            rupu_config::PricingConfig::default(),
        )
    }

    fn install(skills_dir: &FsPath, dir: &str, frontmatter: &str, body: &str) {
        let d = skills_dir.join(dir);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(
            d.join("SKILL.md"),
            format!("---\n{frontmatter}---\n{body}\n"),
        )
        .unwrap();
    }

    fn register_workspace(tmp: &tempfile::TempDir, id: &str, root: &FsPath) {
        std::fs::create_dir_all(tmp.path().join("workspaces")).unwrap();
        std::fs::write(
            tmp.path().join("workspaces").join(format!("{id}.toml")),
            format!(
                "id = \"{id}\"\npath = \"{}\"\ncreated_at = \"2026-01-01T00:00:00Z\"\n",
                root.display()
            ),
        )
        .unwrap();
    }

    async fn get(s: &AppState, uri: &str) -> (StatusCode, serde_json::Value) {
        let resp = routes()
            .with_state(s.clone())
            .oneshot(Request::get(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = resp.status();
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&body).unwrap())
    }

    fn rows(json: &serde_json::Value) -> Vec<serde_json::Value> {
        json["skills"].as_array().expect("skills array").clone()
    }

    #[tokio::test]
    async fn the_global_list_shows_each_skill_and_its_source() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        let global = tmp.path().join("skills");
        install(
            &global,
            "pdf",
            "name: pdf\ndescription: Read PDFs.\n",
            "Use pdftotext.",
        );

        let (status, json) = get(&s, "/api/skills").await;
        assert_eq!(status, StatusCode::OK);
        let r = rows(&json);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0]["name"], "pdf");
        assert_eq!(r[0]["layer"], "global");
        assert_eq!(r[0]["status"], "ok");
        assert_eq!(r[0]["description"], "Read PDFs.");
        assert_eq!(r[0]["source"], serde_json::Value::Null);
        assert_eq!(r[0]["both_layers"], false);
        assert_eq!(r[0]["warnings"], serde_json::json!([]));
        assert!(r[0]["stamp"].as_str().is_some_and(|s| !s.is_empty()));
        assert_eq!(json["warnings"], serde_json::json!([]));

        std::fs::create_dir_all(global.join(".sources")).unwrap();
        std::fs::write(
            global.join(".sources/pdf.json"),
            r#"{"version":1,"source":"github:acme/skills#pdf","content_hash":"ab","installed_at":1}"#,
        )
        .unwrap();
        let (_, json) = get(&s, "/api/skills").await;
        assert_eq!(rows(&json)[0]["source"], "github:acme/skills#pdf");
    }

    #[tokio::test]
    async fn a_project_adds_its_layer_and_marks_names_in_both() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        install(
            &tmp.path().join("skills"),
            "pdf",
            "name: pdf\ndescription: Global PDFs.\n",
            "g",
        );
        let repo = tempfile::TempDir::new().unwrap();
        let project = repo.path().join(".rupu/skills");
        install(
            &project,
            "pdf",
            "name: pdf\ndescription: Project PDFs.\n",
            "p",
        );
        install(&project, "notes", "name: notes\ndescription: Notes.\n", "n");
        register_workspace(&tmp, "ws1", repo.path());

        let (_, json) = get(&s, "/api/skills").await;
        assert_eq!(rows(&json).len(), 1, "no ws_id: the global layer only");

        let (status, json) = get(&s, "/api/skills?ws_id=ws1").await;
        assert_eq!(status, StatusCode::OK);
        let r = rows(&json);
        let keys: Vec<(String, String, bool)> = r
            .iter()
            .map(|r| {
                (
                    r["name"].as_str().unwrap().to_owned(),
                    r["layer"].as_str().unwrap().to_owned(),
                    r["both_layers"].as_bool().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            keys,
            [
                ("notes".into(), "project".into(), false),
                ("pdf".into(), "global".into(), true),
                ("pdf".into(), "project".into(), true),
            ]
        );
        assert_eq!(r[2]["description"], "Project PDFs.");
    }

    #[tokio::test]
    async fn an_invalid_project_skill_lists_with_its_error() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        let repo = tempfile::TempDir::new().unwrap();
        install(
            &repo.path().join(".rupu/skills"),
            "broken",
            "name: other\ndescription: Misnamed.\n",
            "b",
        );
        register_workspace(&tmp, "ws1", repo.path());

        let (status, json) = get(&s, "/api/skills?ws_id=ws1").await;
        assert_eq!(status, StatusCode::OK);
        let r = rows(&json);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0]["name"], "broken");
        assert_eq!(r[0]["status"], "invalid");
        assert_eq!(r[0]["description"], serde_json::Value::Null);
        assert_eq!(r[0]["stamp"], serde_json::Value::Null);
        assert_eq!(
            r[0]["error"],
            "name \"other\" does not match its directory \"broken\""
        );

        let (status, json) = get(&s, "/api/skills/project/broken?ws_id=ws1").await;
        assert_eq!(status, StatusCode::OK, "{json}");
        assert_eq!(json["status"], "invalid");
        assert_eq!(json["body"], serde_json::Value::Null);
    }

    #[tokio::test]
    async fn a_long_description_lists_ok_with_a_warning() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        let long = "a".repeat(1100);
        install(
            &tmp.path().join("skills"),
            "wordy",
            &format!("name: wordy\ndescription: {long}\n"),
            "w",
        );

        let (_, json) = get(&s, "/api/skills").await;
        let r = rows(&json);
        assert_eq!(r[0]["status"], "ok");
        assert_eq!(
            r[0]["warnings"],
            serde_json::json!([
                "`description` is 1100 characters; the agentskills.io limit is 1024"
            ])
        );
    }

    #[tokio::test]
    async fn the_detail_shows_one_skill() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        let global = tmp.path().join("skills");
        install(
            &global,
            "pdf",
            "name: pdf\ndescription: Read PDFs.\nlicense: MIT\nallowed-tools: Bash(pdftotext:*) Read\nmetadata:\n  owner: docs\n",
            "Use pdftotext.",
        );
        std::fs::create_dir_all(global.join("pdf/scripts")).unwrap();
        std::fs::write(global.join("pdf/scripts/extract.sh"), "echo hi\n").unwrap();
        // The stamp reads each file's mode and mtime: fix both.
        for f in ["pdf/SKILL.md", "pdf/scripts/extract.sh"] {
            use std::os::unix::fs::PermissionsExt as _;
            let path = global.join(f);
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
            std::fs::File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_800_000_000))
                .unwrap();
        }

        let (status, json) = get(&s, "/api/skills/global/pdf").await;
        assert_eq!(status, StatusCode::OK, "{json}");
        assert_eq!(json["name"], "pdf");
        assert_eq!(json["layer"], "global");
        assert_eq!(json["status"], "ok");
        assert_eq!(json["body"], "Use pdftotext.");
        assert_eq!(json["files"], serde_json::json!(["scripts/extract.sh"]));
        assert_eq!(json["license"], "MIT");
        assert_eq!(
            json["allowed_tools"],
            serde_json::json!(["Bash(pdftotext:*)", "Read"])
        );
        assert_eq!(
            json["mapped_tools"],
            serde_json::json!(["bash", "read_file"])
        );
        assert_eq!(json["metadata"], serde_json::json!({"owner": "docs"}));
        assert_eq!(json["disable_model_invocation"], false);
        assert_eq!(json["source_record"], serde_json::Value::Null);
        assert_eq!(json["content_hash"], "42b1931a0338");
        assert_eq!(json["stamp"], "27961ea1b0d8");

        let (status, json) = get(&s, "/api/skills/global/nope").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(json["error"], "no installed skill global:nope");

        let (status, _) = get(&s, "/api/skills/team/pdf").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        let (status, _) = get(&s, "/api/skills/project/pdf").await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "a project skill needs ws_id"
        );
    }

    fn write_record(skills_dir: &FsPath, name: &str, text: &str) {
        std::fs::create_dir_all(skills_dir.join(".sources")).unwrap();
        std::fs::write(skills_dir.join(format!(".sources/{name}.json")), text).unwrap();
    }

    #[tokio::test]
    async fn a_project_layer_that_cant_be_read_is_skipped_with_a_warning() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        install(
            &tmp.path().join("skills"),
            "pdf",
            "name: pdf\ndescription: Read PDFs.\n",
            "g",
        );
        let repo = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(repo.path().join(".rupu")).unwrap();
        std::fs::write(repo.path().join(".rupu/skills"), "not a directory").unwrap();
        register_workspace(&tmp, "ws1", repo.path());
        let warning = format!(
            "project layer: {}: Not a directory (os error 20)",
            repo.path().join(".rupu/skills").display()
        );

        let (status, json) = get(&s, "/api/skills?ws_id=ws1").await;
        assert_eq!(status, StatusCode::OK, "{json}");
        let r = rows(&json);
        assert_eq!(r.len(), 1);
        assert_eq!(
            (&r[0]["name"], &r[0]["layer"]),
            (&"pdf".into(), &"global".into())
        );
        assert_eq!(json["warnings"], serde_json::json!([warning]));

        let (status, _) = get(&s, "/api/skills/global/pdf?ws_id=ws1").await;
        assert_eq!(status, StatusCode::OK);
        let (status, json) = get(&s, "/api/skills/project/pdf?ws_id=ws1").await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(json["error"], warning);
    }

    #[tokio::test]
    async fn a_corrupt_install_record_is_a_warning() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        let global = tmp.path().join("skills");
        install(&global, "pdf", "name: pdf\ndescription: Read PDFs.\n", "g");
        write_record(&global, "pdf", "{not json");
        let error = format!(
            "{}: key must be a string at line 1 column 2",
            global.join(".sources/pdf.json").display()
        );

        let (status, json) = get(&s, "/api/skills").await;
        assert_eq!(status, StatusCode::OK);
        let r = rows(&json);
        assert_eq!(r[0]["status"], "ok");
        assert_eq!(r[0]["source"], serde_json::Value::Null);
        assert_eq!(
            r[0]["warnings"],
            serde_json::json!([format!("install record: {error}")])
        );

        let (_, json) = get(&s, "/api/skills/global/pdf").await;
        assert_eq!(json["source_record"], serde_json::Value::Null);
        assert_eq!(json["source_error"], error);
    }

    #[tokio::test]
    async fn an_oversized_install_record_is_not_read() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        let global = tmp.path().join("skills");
        install(&global, "pdf", "name: pdf\ndescription: Read PDFs.\n", "g");
        write_record(&global, "pdf", &" ".repeat(64 * 1024 + 1));

        let (_, json) = get(&s, "/api/skills/global/pdf").await;
        assert_eq!(
            json["source_error"],
            format!(
                "{}: the record is over 65536 bytes",
                global.join(".sources/pdf.json").display()
            )
        );
    }

    #[tokio::test]
    async fn an_oversized_skill_lists_invalid() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        install(
            &tmp.path().join("skills"),
            "big",
            "name: big\ndescription: Big.\n",
            &"a".repeat(1 << 20),
        );

        let (status, json) = get(&s, "/api/skills").await;
        assert_eq!(status, StatusCode::OK);
        let r = rows(&json);
        assert_eq!(r[0]["status"], "invalid");
        assert_eq!(
            r[0]["error"],
            "SKILL.md is 1048613 bytes; the limit is 1048576 bytes"
        );
        let (status, json) = get(&s, "/api/skills/global/big").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["status"], "invalid");
        assert_eq!(json["body"], serde_json::Value::Null);
    }

    #[tokio::test]
    async fn a_valid_project_skill_shows_its_body() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        let repo = tempfile::TempDir::new().unwrap();
        install(
            &repo.path().join(".rupu/skills"),
            "notes",
            "name: notes\ndescription: Notes.\n",
            "Write notes.",
        );
        register_workspace(&tmp, "ws1", repo.path());

        let (status, json) = get(&s, "/api/skills/project/notes?ws_id=ws1").await;
        assert_eq!(status, StatusCode::OK, "{json}");
        assert_eq!(json["layer"], "project");
        assert_eq!(json["status"], "ok");
        assert_eq!(json["body"], "Write notes.");
        assert_eq!(json["description"], "Notes.");
    }

    /// A symlinked project skill, or a symlinked project `skills/`, lists
    /// invalid; the linked root's records are not read.
    #[cfg(unix)]
    #[tokio::test]
    async fn symlinked_project_skills_list_invalid() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        let outside = tempfile::TempDir::new().unwrap();
        install(
            outside.path(),
            "notes",
            "name: notes\ndescription: Notes.\n",
            "n",
        );
        write_record(outside.path(), "notes", "{not json");
        let repo = tempfile::TempDir::new().unwrap();
        let skills = repo.path().join(".rupu/skills");
        std::fs::create_dir_all(&skills).unwrap();
        std::os::unix::fs::symlink(outside.path().join("notes"), skills.join("notes")).unwrap();
        register_workspace(&tmp, "ws1", repo.path());

        let (_, json) = get(&s, "/api/skills?ws_id=ws1").await;
        let r = rows(&json);
        assert_eq!(r[0]["status"], "invalid");
        assert_eq!(
            r[0]["error"],
            "a project skill directory may not be a symlink"
        );

        let repo = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(repo.path().join(".rupu")).unwrap();
        std::os::unix::fs::symlink(outside.path(), repo.path().join(".rupu/skills")).unwrap();
        register_workspace(&tmp, "ws2", repo.path());

        let (_, json) = get(&s, "/api/skills?ws_id=ws2").await;
        let r = rows(&json);
        assert_eq!(r[0]["status"], "invalid");
        assert_eq!(
            r[0]["error"],
            "the project skills directory may not be a symlink"
        );
        assert_eq!(r[0]["warnings"], serde_json::json!([]), "no record read");
        let (_, json) = get(&s, "/api/skills/project/notes?ws_id=ws2").await;
        assert_eq!(
            json["source_error"],
            "the project skills directory may not be a symlink"
        );
    }

    #[tokio::test]
    async fn an_unknown_workspace_is_not_found() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        let (status, _) = get(&s, "/api/skills?ws_id=nope").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = get(&s, "/api/skills/global/pdf?ws_id=nope").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    fn packed(tmp: &tempfile::TempDir) -> rupu_skills::ship::PackedSkill {
        let dir = tmp.path().join("src");
        install(&dir, "pdf", "name: pdf\ndescription: PDFs.\n", "Body.");
        rupu_skills::ship::pack_skill(
            &rupu_skills::load_skill(&dir.join("pdf"), Layer::Global).unwrap(),
        )
        .unwrap()
    }

    async fn send(s: &AppState, req: Request<Body>) -> (StatusCode, serde_json::Value) {
        let resp = routes().with_state(s.clone()).oneshot(req).await.unwrap();
        let status = resp.status();
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null),
        )
    }

    fn missing_req(r: &ShippedSkillRef) -> Request<Body> {
        Request::post("/api/skills/bundle/missing")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::json!({ "skills": [r] }).to_string()))
            .unwrap()
    }

    fn put_req(hash: &str, name: &str, bytes: Vec<u8>) -> Request<Body> {
        Request::put(format!(
            "/api/skills/bundle/{hash}?layer=global&name={name}"
        ))
        .body(Body::from(bytes))
        .unwrap()
    }

    #[tokio::test]
    async fn missing_then_put_then_missing_is_empty() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        let p = packed(&tmp);
        let r = p.reference();

        let (st, json) = send(&s, missing_req(&r)).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(json["missing"].as_array().unwrap().len(), 1);

        let (st, json) = send(&s, put_req(&r.content_hash, "pdf", p.bytes.clone())).await;
        assert_eq!(st, StatusCode::OK, "{json}");
        assert_eq!(json["stored"], r.content_hash);

        let (_, json) = send(&s, missing_req(&r)).await;
        assert_eq!(json["missing"], serde_json::json!([]));
    }

    #[tokio::test]
    async fn a_tampered_body_or_hostile_name_is_a_400() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        let p = packed(&tmp);
        let r = p.reference();

        let wrong = "0".repeat(64);
        let (st, _) = send(&s, put_req(&wrong, "pdf", p.bytes.clone())).await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        let (st, _) = send(&s, put_req(&r.content_hash, "pdf", b"not a tar".to_vec())).await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        let (st, _) = send(&s, put_req(&r.content_hash, "..%2Fx", p.bytes.clone())).await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        let (_, json) = send(&s, missing_req(&r)).await;
        assert_eq!(
            json["missing"].as_array().unwrap().len(),
            1,
            "nothing cached"
        );
    }

    #[tokio::test]
    async fn the_bundle_routes_leave_the_skill_detail_route_serving() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        install(
            &tmp.path().join("skills"),
            "pdf",
            "name: pdf\ndescription: Read PDFs.\n",
            "Body.",
        );
        let (st, json) = get(&s, "/api/skills/global/pdf").await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(json["name"], "pdf");
    }
}

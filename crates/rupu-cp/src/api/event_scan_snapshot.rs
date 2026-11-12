//! Exact active-store owners for bounded event scans. No run catalog, legacy
//! enrichment, workspace enumeration or unbounded metadata read is used here.
use crate::{
    api::fs_open::open_anchored_regular_file,
    error::{ApiError, ApiResult},
};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, Metadata},
    io::{self, Read},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

pub(crate) const MAX_METADATA_BYTES: u64 = 1024 * 1024;
const MAX_RECORD_BYTES: u64 = 32 * 1024;
const MAX_MANIFEST_BYTES: usize = 32 * 1024;

fn error(status: StatusCode, code: &str, message: &str) -> ApiError {
    ApiError(status, format!("{code}: {message}"))
}
fn invalid(message: &str) -> ApiError {
    error(StatusCode::BAD_REQUEST, "events_invalid_selection", message)
}
fn changed() -> ApiError {
    error(
        StatusCode::CONFLICT,
        "events_snapshot_changed",
        "recorded files changed; restart without a cursor",
    )
}
fn owner_mismatch() -> ApiError {
    error(
        StatusCode::CONFLICT,
        "events_owner_mismatch",
        "recorded run or workspace identity does not match the requested owner",
    )
}
fn nanos(time: SystemTime) -> Option<u128> {
    time.duration_since(UNIX_EPOCH).ok().map(|d| d.as_nanos())
}
/// Logical path observations, not raw syscall counts inside the anchored walker.
#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct SnapshotStats {
    pub(crate) file_observations: u64,
    pub(crate) root_checks: u64,
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Stamp {
    pub(crate) len: u64,
    modified: u128,
    created: Option<u128>,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(unix)]
    changed: (i64, i64),
}
impl Stamp {
    fn of(metadata: &Metadata) -> ApiResult<Self> {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        Ok(Self {
            len: metadata.len(),
            modified: metadata
                .modified()
                .ok()
                .and_then(nanos)
                .ok_or_else(changed)?,
            created: metadata.created().ok().and_then(nanos),
            #[cfg(unix)]
            device: metadata.dev(),
            #[cfg(unix)]
            inode: metadata.ino(),
            #[cfg(unix)]
            changed: (metadata.ctime(), metadata.ctime_nsec()),
        })
    }
    fn timestamp_ms(&self) -> Option<i64> {
        i64::try_from(self.modified / 1_000_000).ok()
    }
}

/// Directory identity deliberately excludes mtime: unrelated newly created
/// runs do not invalidate this explicit selected-run snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct DirectoryIdentity {
    created: Option<u128>,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}
impl DirectoryIdentity {
    fn of(metadata: &Metadata) -> ApiResult<Self> {
        if !metadata.is_dir() {
            return Err(changed());
        }
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        Ok(Self {
            created: metadata.created().ok().and_then(nanos),
            #[cfg(unix)]
            device: metadata.dev(),
            #[cfg(unix)]
            inode: metadata.ino(),
        })
    }
}
#[derive(Debug, Clone, Serialize)]
struct Anchor {
    supplied: PathBuf,
    canonical: PathBuf,
    identity: Option<DirectoryIdentity>,
}
impl Anchor {
    fn capture(path: &Path, allow_missing: bool) -> ApiResult<Self> {
        if !path.is_absolute() || path.as_os_str().len() > 4096 || path.components().count() > 64 {
            return Err(invalid("invalid configured store root"));
        }
        match path.canonicalize() {
            Ok(canonical) => {
                if canonical.as_os_str().len() > 4096 || canonical.components().count() > 64 {
                    return Err(invalid("configured root exceeds path budget"));
                }
                let identity =
                    DirectoryIdentity::of(&fs::metadata(&canonical).map_err(|_| changed())?)?;
                Ok(Self {
                    supplied: path.to_owned(),
                    canonical,
                    identity: Some(identity),
                })
            }
            Err(e) if allow_missing && e.kind() == io::ErrorKind::NotFound => {
                let parent = path
                    .parent()
                    .ok_or_else(|| invalid("invalid store root"))?
                    .canonicalize()
                    .map_err(|_| changed())?;
                let name = path
                    .file_name()
                    .ok_or_else(|| invalid("invalid store root"))?;
                let canonical = parent.join(name);
                if canonical.as_os_str().len() > 4096 || canonical.components().count() > 64 {
                    return Err(invalid("canonical store root exceeds the path budget"));
                }
                if fs::symlink_metadata(&canonical).is_ok() {
                    return Err(changed());
                }
                Ok(Self {
                    supplied: path.to_owned(),
                    canonical,
                    identity: None,
                })
            }
            Err(_) => Err(changed()),
        }
    }
    fn validate(&self) -> ApiResult<()> {
        let actual =
            Self::capture(&self.supplied, self.identity.is_none()).map_err(|_| changed())?;
        if actual.canonical != self.canonical || actual.identity != self.identity {
            return Err(changed());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
enum Observation {
    File(Stamp),
    /// No content claim is made for inaccessible files. The observed failure
    /// class is bound; becoming available or changing class invalidates.
    Unavailable(String),
}
fn observe(path: &Path) -> ApiResult<(Observation, Option<File>)> {
    match open_anchored_regular_file(path) {
        Ok(file) => {
            let stamp = Stamp::of(&file.metadata().map_err(|_| changed())?)?;
            Ok((Observation::File(stamp), Some(file)))
        }
        Err(e) => Ok((Observation::Unavailable(format!("{:?}", e.kind())), None)),
    }
}
#[derive(Debug, Clone, Serialize)]
struct Watch {
    in_global: bool,
    relative: PathBuf,
    observation: Observation,
}

/// Only the header is retained. Derived decoding rejects duplicate owner
/// keys; unknown run payload fields stay ignored under the input byte limit.
#[derive(Deserialize)]
struct RunHeader {
    id: Option<String>,
    workspace_id: Option<String>,
    workflow_name: Option<String>,
    workspace_path: Option<String>,
    started_at: Option<String>,
    codename: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct UnavailableRun {
    pub(crate) run_id: String,
    pub(crate) reason: String,
}
#[derive(Debug, Clone, Serialize)]
pub(crate) struct SnapshotRun {
    pub(crate) id: String,
    pub(crate) workspace_id: String,
    pub(crate) workflow_name: String,
    pub(crate) project_name: String,
    pub(crate) run_codename: String,
    pub(crate) started_at_ms: i64,
    started_at_key: (i64, u32),
    pub(crate) event_stamp: Stamp,
    pub(crate) fallback_timestamp_ms: i64,
    pub(crate) fallback_timestamp_source: &'static str,
}
#[derive(Debug, Clone, Serialize)]
pub(crate) struct Manifest {
    global: Anchor,
    root: Anchor,
    watches: Vec<Watch>,
    pub(crate) runs: Vec<SnapshotRun>,
    pub(crate) unavailable: Vec<UnavailableRun>,
    pub(crate) metadata_bytes_read: u64,
    pub(crate) capture_stats: SnapshotStats,
}

impl Manifest {
    pub(crate) fn capture(
        global: &Path,
        root: &Path,
        ids: &[String],
        workspace: Option<&str>,
    ) -> ApiResult<Self> {
        if ids.is_empty()
            || ids.len() > 20
            || ids.iter().any(|id| !valid_id(id))
            || ids.windows(2).any(|pair| pair[0] >= pair[1])
            || workspace.is_some_and(|id| !valid_id(id))
        {
            return Err(invalid(
                "use one to twenty distinct sorted exact run IDs and a valid optional workspace ID",
            ));
        }
        let mut manifest = Self {
            global: Anchor::capture(global, false)?,
            root: Anchor::capture(root, true)?,
            watches: Vec::new(),
            runs: Vec::new(),
            unavailable: Vec::new(),
            metadata_bytes_read: 0,
            capture_stats: SnapshotStats {
                file_observations: 0,
                root_checks: 2,
            },
        };
        // A separate configured run root is supported and bound explicitly; it
        // is never resolved from an event or run's recorded workspace path.
        if let Some(workspace) = workspace {
            manifest.capture_workspace(workspace)?;
        }
        for id in ids {
            manifest.capture_run(id, workspace, ids.len() == 1)?;
        }
        manifest.runs.sort_by(|a, b| {
            b.started_at_key
                .cmp(&a.started_at_key)
                .then_with(|| b.id.cmp(&a.id))
        });
        // Includes files not yet traversed and unavailable observations.
        let mut stats = SnapshotStats::default();
        drop(manifest.validate_counted(&mut stats)?);
        manifest.capture_stats.file_observations += stats.file_observations;
        manifest.capture_stats.root_checks += stats.root_checks;
        if serde_json::to_vec(&manifest)
            .map_err(|_| invalid("cannot encode selected owner manifest"))?
            .len()
            > MAX_MANIFEST_BYTES
        {
            return Err(invalid(
                "selected owner metadata exceeds the manifest size budget; select fewer runs",
            ));
        }
        Ok(manifest)
    }

    fn watch(
        &mut self,
        in_global: bool,
        relative: PathBuf,
    ) -> ApiResult<(Observation, Option<File>)> {
        let base = if in_global {
            &self.global.canonical
        } else {
            &self.root.canonical
        };
        self.capture_stats.file_observations += 1;
        let (observation, file) = observe(&base.join(&relative))?;
        self.watches.push(Watch {
            in_global,
            relative,
            observation: observation.clone(),
        });
        Ok((observation, file))
    }
    fn read_metadata(&mut self, mut file: File, stamp: &Stamp) -> ApiResult<Option<Vec<u8>>> {
        if stamp.len > MAX_RECORD_BYTES {
            return Ok(None);
        }
        let mut bytes = Vec::with_capacity(stamp.len as usize);
        let mut buffer = [0_u8; 4096];
        // Read exactly the observed length, then recheck the handle stamp.
        // Actual bytes are charged before parsing; no read-ahead escapes the bound.
        let mut remaining = stamp.len;
        while remaining > 0 {
            let allowance = MAX_METADATA_BYTES.saturating_sub(self.metadata_bytes_read);
            if allowance == 0 {
                return Err(invalid("metadata read budget exhausted"));
            }
            let length = usize::try_from(remaining.min(allowance).min(buffer.len() as u64))
                .unwrap_or(buffer.len());
            let count = file.read(&mut buffer[..length]).map_err(|_| {
                error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "events_metadata_unreadable",
                    "recorded metadata could not be read",
                )
            })?;
            if count == 0 {
                break;
            }
            self.metadata_bytes_read += count as u64;
            remaining -= count as u64;
            bytes.extend_from_slice(&buffer[..count]);
        }
        if Stamp::of(&file.metadata().map_err(|_| changed())?)? != *stamp
            || bytes.len() as u64 != stamp.len
        {
            return Err(changed());
        }
        Ok(Some(bytes))
    }
    fn capture_workspace(&mut self, workspace: &str) -> ApiResult<()> {
        let (observation, file) = self.watch(
            true,
            PathBuf::from("workspaces").join(format!("{workspace}.toml")),
        )?;
        let (Observation::File(stamp), Some(file)) = (observation, file) else {
            return Err(error(
                StatusCode::NOT_FOUND,
                "events_missing_workspace",
                "requested workspace registration is unavailable",
            ));
        };
        let Some(bytes) = self.read_metadata(file, &stamp)? else {
            return Err(error(
                StatusCode::PAYLOAD_TOO_LARGE,
                "events_workspace_unavailable",
                "workspace registration exceeds the metadata budget",
            ));
        };
        let text = std::str::from_utf8(&bytes)
            .map_err(|_| invalid("workspace registration is not valid UTF-8"))?;
        let record: rupu_workspace::Workspace =
            toml::from_str(text).map_err(|_| invalid("workspace registration is malformed"))?;
        if record.id != workspace {
            return Err(owner_mismatch());
        }
        Ok(())
    }
    fn unavailable(&mut self, id: &str, reason: &str) {
        self.unavailable.push(UnavailableRun {
            run_id: id.to_owned(),
            reason: reason.to_owned(),
        });
    }
    fn capture_run(&mut self, id: &str, workspace: Option<&str>, single: bool) -> ApiResult<()> {
        let (observation, file) = self.watch(false, PathBuf::from(id).join("run.json"))?;
        let (stamp, file) = match (observation, file) {
            (Observation::File(stamp), Some(file)) => (stamp, file),
            (Observation::Unavailable(kind), _) => {
                if single && kind == "NotFound" {
                    return Err(error(
                        StatusCode::NOT_FOUND,
                        "events_missing_run",
                        "requested run is absent from the active recorded store",
                    ));
                }
                self.unavailable(
                    id,
                    if kind == "NotFound" {
                        "run_not_in_active_store"
                    } else {
                        "run_metadata_unreadable"
                    },
                );
                return Ok(());
            }
            _ => return Err(changed()),
        };
        let bytes = match self.read_metadata(file, &stamp) {
            Ok(Some(bytes)) => bytes,
            Ok(None) => {
                self.unavailable(id, "run_metadata_oversized");
                return Ok(());
            }
            Err(error) if error.1.starts_with("events_metadata_unreadable:") => {
                self.unavailable(id, "run_metadata_unreadable");
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        let Ok(record) = serde_json::from_slice::<RunHeader>(&bytes) else {
            self.unavailable(id, "run_metadata_malformed");
            return Ok(());
        };
        if record.id.as_deref().is_some_and(|actual| actual != id)
            || workspace.is_some_and(|expected| {
                record
                    .workspace_id
                    .as_deref()
                    .is_some_and(|actual| actual != expected)
            })
        {
            return Err(owner_mismatch());
        }
        let (
            Some(actual_id),
            Some(workspace_id),
            Some(workflow_name),
            Some(workspace_path),
            Some(started_at),
        ) = (
            record.id.as_deref(),
            record.workspace_id.as_deref(),
            record.workflow_name.as_deref(),
            record.workspace_path.as_deref(),
            record
                .started_at
                .as_deref()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok()),
        )
        else {
            self.unavailable(id, "run_metadata_incomplete");
            return Ok(());
        };
        let codename = record.codename.as_deref();
        if actual_id != id
            || !valid_id(workspace_id)
            || workflow_name.len() > 512
            || workspace_path.len() > 4096
            || codename.is_some_and(|value| value.len() > 128)
        {
            self.unavailable(id, "run_metadata_invalid");
            return Ok(());
        }
        let project_name = Path::new(workspace_path)
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        if project_name.len() > 256 {
            self.unavailable(id, "run_metadata_invalid");
            return Ok(());
        }
        let (observation, file) = self.watch(false, PathBuf::from(id).join("events.jsonl"))?;
        let (Observation::File(event_stamp), Some(_)) = (observation, file) else {
            self.unavailable(id, "events_unavailable");
            return Ok(());
        };
        let started_at_ms = started_at.timestamp_millis();
        let fallback = event_stamp.timestamp_ms();
        self.runs.push(SnapshotRun {
            id: id.to_owned(),
            workspace_id: workspace_id.to_owned(),
            workflow_name: workflow_name.to_owned(),
            project_name,
            run_codename: crate::codename::named(codename, id, None).0,
            started_at_ms,
            started_at_key: (started_at.timestamp(), started_at.timestamp_subsec_nanos()),
            fallback_timestamp_ms: fallback.unwrap_or(started_at_ms),
            fallback_timestamp_source: if fallback.is_some() {
                "snapshot_file_mtime"
            } else {
                "run_started_at"
            },
            event_stamp,
        });
        Ok(())
    }

    /// No metadata content is reread: unchanged stamps retain the bounded
    /// authoritative ownership read. All selected paths (including unavailable
    /// and not-yet-traversed files) are checked on every page, twice by caller.
    /// Returned handles follow `runs` order and must be dropped after the page.
    #[cfg(test)]
    pub(crate) fn validate(&self) -> ApiResult<Vec<File>> {
        self.validate_counted(&mut SnapshotStats::default())
    }

    pub(crate) fn validate_counted(&self, stats: &mut SnapshotStats) -> ApiResult<Vec<File>> {
        stats.root_checks += 2;
        self.global.validate()?;
        self.root.validate()?;
        let mut handles = std::collections::HashMap::new();
        for watch in &self.watches {
            let base = if watch.in_global {
                &self.global.canonical
            } else {
                &self.root.canonical
            };
            stats.file_observations += 1;
            let (actual, file) = observe(&base.join(&watch.relative))?;
            if actual != watch.observation {
                return Err(changed());
            }
            if !watch.in_global
                && watch
                    .relative
                    .file_name()
                    .is_some_and(|name| name == "events.jsonl")
            {
                if let Some(file) = file {
                    handles.insert(watch.relative.clone(), file);
                }
            }
        }
        stats.root_checks += 2;
        self.global.validate()?;
        self.root.validate()?;
        self.runs
            .iter()
            .map(|run| {
                handles
                    .remove(&PathBuf::from(&run.id).join("events.jsonl"))
                    .ok_or_else(changed)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    use std::io::Write;
    fn seed(root: &Path, id: &str, workspace: &str, started: &str) -> u64 {
        let path = root.join(id);
        fs::create_dir_all(&path).unwrap();
        let bytes = serde_json::to_vec(&json!({
            "id": id, "workspace_id": workspace, "workspace_path": "/projects/shared-name",
            "workflow_name": "scan", "started_at": started,
        }))
        .unwrap();
        fs::write(path.join("run.json"), &bytes).unwrap();
        fs::write(path.join("events.jsonl"), b"{\"type\":\"test\"}\n").unwrap();
        bytes.len() as u64
    }
    fn workspace(global: &Path, id: &str) {
        fs::create_dir_all(global.join("workspaces")).unwrap();
        fs::write(global.join("workspaces").join(format!("{id}.toml")),
            format!("id = \"{id}\"\npath = \"/projects/shared-name\"\ncreated_at = \"2026-11-09T00:00:00Z\"\n")).unwrap();
    }
    fn ids(values: &[&str]) -> Vec<String> {
        values.iter().map(|s| (*s).to_owned()).collect()
    }
    #[test]
    fn capture_reads_only_selected_metadata_and_orders_runs() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("runs");
        let a = seed(&root, "run_a", "ws_a", "2026-11-08T00:00:00Z");
        let b = seed(&root, "run_b", "ws_a", "2026-11-09T00:00:00Z");
        // Unselected garbage must never be enumerated or read.
        fs::create_dir_all(root.join("unselected")).unwrap();
        fs::write(root.join("unselected/run.json"), vec![0; 128 * 1024]).unwrap();
        let snapshot =
            Manifest::capture(temp.path(), &root, &ids(&["run_a", "run_b"]), None).unwrap();
        assert_eq!(
            snapshot
                .runs
                .iter()
                .map(|r| r.id.as_str())
                .collect::<Vec<_>>(),
            ["run_b", "run_a"]
        );
        assert_eq!(snapshot.metadata_bytes_read, a + b);
        assert_eq!(snapshot.capture_stats.file_observations, 8); // capture + unchanged check
        assert_eq!(snapshot.capture_stats.root_checks, 6);
        let mut counts = SnapshotStats::default();
        assert_eq!(snapshot.validate_counted(&mut counts).unwrap().len(), 2);
        assert_eq!(counts.file_observations, 4);
        assert_eq!(counts.root_checks, 4);
    }

    #[test]
    fn ordering_preserves_submillisecond_recorded_start_precision() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("runs");
        seed(&root, "run_a", "ws_a", "2026-11-09T00:00:00.000900Z");
        seed(&root, "run_z", "ws_a", "2026-11-09T00:00:00.000100Z");
        let snapshot =
            Manifest::capture(temp.path(), &root, &ids(&["run_a", "run_z"]), None).unwrap();
        assert_eq!(
            snapshot
                .runs
                .iter()
                .map(|r| r.id.as_str())
                .collect::<Vec<_>>(),
            ["run_a", "run_z"]
        );
    }
    #[test]
    fn missing_single_is_404_but_multi_is_qualified_and_watched() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("runs");
        seed(&root, "run_a", "ws_a", "2026-11-09T00:00:00Z");
        let error =
            Manifest::capture(temp.path(), &root, &ids(&["run_missing"]), None).unwrap_err();
        assert_eq!(error.0, StatusCode::NOT_FOUND);
        let snapshot =
            Manifest::capture(temp.path(), &root, &ids(&["run_a", "run_missing"]), None).unwrap();
        assert_eq!(snapshot.runs.len(), 1);
        assert_eq!(snapshot.unavailable[0].reason, "run_not_in_active_store");
        snapshot.validate().unwrap();
        seed(&root, "run_missing", "ws_a", "2026-11-09T00:00:00Z");
        assert_eq!(snapshot.validate().unwrap_err().0, StatusCode::CONFLICT);
    }
    #[test]
    fn absent_store_is_not_an_archive_fallback() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("runs");
        seed(
            &temp.path().join("runs-archive"),
            "run_a",
            "ws_a",
            "2026-11-09T00:00:00Z",
        );
        assert_eq!(
            Manifest::capture(temp.path(), &root, &ids(&["run_a"]), None)
                .unwrap_err()
                .0,
            StatusCode::NOT_FOUND
        );
        let snapshot =
            Manifest::capture(temp.path(), &root, &ids(&["run_a", "run_b"]), None).unwrap();
        assert!(snapshot.runs.is_empty());
        assert_eq!(snapshot.unavailable.len(), 2);
        snapshot.validate().unwrap();
        fs::create_dir_all(&root).unwrap();
        assert_eq!(snapshot.validate().unwrap_err().0, StatusCode::CONFLICT);
    }
    #[test]
    fn exact_workspace_and_run_identity_mismatches_reject_whole_selection() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("runs");
        workspace(temp.path(), "ws_a");
        seed(&root, "run_a", "ws_a", "2026-11-09T00:00:00Z");
        seed(&root, "run_b", "ws_b", "2026-11-09T00:00:00Z");
        let error = Manifest::capture(temp.path(), &root, &ids(&["run_a", "run_b"]), Some("ws_a"))
            .unwrap_err();
        assert!(error.1.starts_with("events_owner_mismatch:"));
        // The equal directory basename never establishes matching ownership.
        let path = root.join("run_a/run.json");
        let mut raw: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        raw["id"] = json!("run_other");
        fs::write(&path, serde_json::to_vec(&raw).unwrap()).unwrap();
        assert!(
            Manifest::capture(temp.path(), &root, &ids(&["run_a"]), None)
                .unwrap_err()
                .1
                .starts_with("events_owner_mismatch:")
        );
    }

    #[test]
    fn duplicate_owner_keys_never_choose_a_last_winner() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("runs");
        seed(&root, "run_a", "ws_a", "2026-11-09T00:00:00Z");
        let path = root.join("run_a/run.json");
        for key in ["id", "workspace_id"] {
            let bytes = format!(r#"{{"{key}":"wrong","{key}":"right"}}"#);
            fs::write(&path, bytes).unwrap();
            let snapshot = Manifest::capture(temp.path(), &root, &ids(&["run_a"]), None).unwrap();
            assert!(snapshot.runs.is_empty());
            assert_eq!(snapshot.unavailable[0].reason, "run_metadata_malformed");
        }
    }
    #[test]
    fn oversized_and_malformed_metadata_are_bounded_unavailable() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("runs");
        seed(&root, "run_large", "ws_a", "2026-11-09T00:00:00Z");
        seed(&root, "run_bad", "ws_a", "2026-11-09T00:00:00Z");
        fs::write(
            root.join("run_large/run.json"),
            vec![b'x'; MAX_RECORD_BYTES as usize + 1],
        )
        .unwrap();
        fs::write(root.join("run_bad/run.json"), b"not JSON").unwrap();
        let snapshot =
            Manifest::capture(temp.path(), &root, &ids(&["run_bad", "run_large"]), None).unwrap();
        assert_eq!(snapshot.metadata_bytes_read, 8);
        assert!(snapshot.runs.is_empty());
        assert_eq!(
            snapshot
                .unavailable
                .iter()
                .map(|u| u.reason.as_str())
                .collect::<Vec<_>>(),
            ["run_metadata_malformed", "run_metadata_oversized"]
        );
        snapshot.validate().unwrap();
    }
    #[test]
    fn any_selected_file_change_invalidates_even_before_its_scan() {
        for kind in [
            "append",
            "truncate",
            "replace",
            "same_length",
            "metadata",
            "archive",
        ] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().join("runs");
            seed(&root, "run_a", "ws_a", "2026-11-09T00:00:00Z");
            seed(&root, "run_b", "ws_a", "2026-11-08T00:00:00Z");
            let snapshot =
                Manifest::capture(temp.path(), &root, &ids(&["run_a", "run_b"]), None).unwrap();
            let path = root.join("run_b/events.jsonl");
            match kind {
                "append" => {
                    fs::OpenOptions::new()
                        .append(true)
                        .open(&path)
                        .unwrap()
                        .write_all(b"next\n")
                        .unwrap();
                }
                "truncate" => {
                    fs::write(&path, b"").unwrap();
                }
                "replace" => {
                    let replacement = root.join("new");
                    fs::write(&replacement, fs::read(&path).unwrap()).unwrap();
                    fs::rename(replacement, &path).unwrap();
                }
                "same_length" => {
                    let len = fs::metadata(&path).unwrap().len() as usize;
                    fs::write(&path, vec![b' '; len]).unwrap();
                }
                "metadata" => {
                    let metadata = root.join("run_b/run.json");
                    let mut bytes = fs::read(&metadata).unwrap();
                    bytes.push(b' ');
                    fs::write(metadata, bytes).unwrap();
                }
                "archive" => {
                    fs::rename(root.join("run_b"), temp.path().join("archived_b")).unwrap();
                }
                _ => unreachable!(),
            }
            assert_eq!(
                snapshot.validate().unwrap_err().0,
                StatusCode::CONFLICT,
                "{kind}"
            );
        }
    }
    #[test]
    fn registration_and_root_changes_are_bound_but_unrelated_runs_are_not() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("runs");
        workspace(temp.path(), "ws_a");
        seed(&root, "run_a", "ws_a", "2026-11-09T00:00:00Z");
        let snapshot =
            Manifest::capture(temp.path(), &root, &ids(&["run_a"]), Some("ws_a")).unwrap();
        seed(&root, "run_unrelated", "ws_other", "2026-11-09T00:00:00Z");
        snapshot.validate().unwrap();
        fs::remove_file(temp.path().join("workspaces/ws_a.toml")).unwrap();
        assert_eq!(snapshot.validate().unwrap_err().0, StatusCode::CONFLICT);
        let snapshot = Manifest::capture(temp.path(), &root, &ids(&["run_a"]), None).unwrap();
        fs::rename(&root, temp.path().join("old_runs")).unwrap();
        seed(&root, "run_a", "ws_a", "2026-11-09T00:00:00Z");
        assert_eq!(snapshot.validate().unwrap_err().0, StatusCode::CONFLICT);
    }
    #[cfg(unix)]
    #[test]
    fn symlinked_parent_and_final_artifacts_are_never_followed() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("runs");
        fs::create_dir_all(&root).unwrap();
        let outside = temp.path().join("outside");
        seed(&outside, "run_a", "ws_a", "2026-11-09T00:00:00Z");
        symlink(outside.join("run_a"), root.join("run_a")).unwrap();
        let snapshot =
            Manifest::capture(temp.path(), &root, &ids(&["run_a", "run_b"]), None).unwrap();
        assert!(snapshot.runs.is_empty());
        assert_eq!(snapshot.metadata_bytes_read, 0);
    }
}

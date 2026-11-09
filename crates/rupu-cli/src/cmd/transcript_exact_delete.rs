//! Exact opt-in standalone transcript deletion. No cwd lookup, fragment/codename
//! fallback or recursive global cleanup. Files are pinned and rechecked under
//! the existing lifecycle lease; this is advisory coordination, not atomic CAS
//! against arbitrary writers. The bounded revision hashes metadata and prologue,
//! not the complete contents of potentially very large transcript files.
use rupu_cp::transcript_mutator::{
    ExactTranscriptContext, ExactTranscriptEffect, ExactTranscriptLimits, ExactTranscriptScope,
};
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum ExactDeleteError {
    #[error("exact_transcript_conflict: {0}")]
    Conflict(&'static str),
    #[error("exact_transcript_not_found: {0}")]
    NotFound(&'static str),
    #[error("exact_transcript_unavailable: {0}")]
    Unavailable(&'static str),
    #[error("exact_transcript_failed: {0}")]
    Failed(&'static str),
}
type Result<T> = std::result::Result<T, ExactDeleteError>;
const SCHEMA: &str = "transcript.delete.exact.v1";
const COORDINATION: &str = "advisory_retention_lease_metadata_revision";

pub fn prepare(
    global: &Path,
    id: &str,
    scope: &ExactTranscriptScope,
) -> Result<ExactTranscriptContext> {
    validate_selection(id, scope)?;
    #[cfg(unix)]
    {
        let result = unix::capture_with_lease(global, id, scope);
        let context = match result {
            Ok(snapshot) => snapshot.context(id, scope)?,
            Err(ExactDeleteError::Unavailable(reason)) => unavailable(id, scope, reason),
            Err(error) => return Err(error),
        };
        bounded(context)
    }
    #[cfg(not(unix))]
    {
        let _ = global;
        Ok(unavailable(
            id,
            scope,
            "exact deletion requires Unix descriptor and lifecycle guards",
        ))
    }
}

pub fn delete(global: &Path, id: &str, scope: &ExactTranscriptScope, revision: &str) -> Result<()> {
    validate_selection(id, scope)?;
    if revision.len() != 71
        || !revision.starts_with("sha256:")
        || !revision[7..]
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(ExactDeleteError::Conflict("invalid reviewed revision"));
    }
    #[cfg(unix)]
    {
        unix::delete(global, id, scope, revision)
    }
    #[cfg(not(unix))]
    {
        let _ = global;
        Err(ExactDeleteError::Unavailable(
            "exact deletion requires Unix descriptor and lifecycle guards",
        ))
    }
}

fn validate_selection(id: &str, scope: &ExactTranscriptScope) -> Result<()> {
    let canonical = id
        .strip_prefix("run_")
        .and_then(|s| s.parse::<ulid::Ulid>().ok())
        .map(|u| format!("run_{u}"));
    if canonical.as_deref() != Some(id) {
        return Err(ExactDeleteError::Conflict(
            "a full canonical run ID is required",
        ));
    }
    if rupu_workspace::customers::validate_ws_id(&scope.workspace_id).is_err()
        || scope.workspace_id.len() > 128
    {
        return Err(ExactDeleteError::Conflict(
            "an exact registered workspace ID is required",
        ));
    }
    if scope.path.len() > 4096
        || scope.path.chars().any(char::is_control)
        || !Path::new(&scope.path).is_absolute()
        || Path::new(&scope.path).components().any(|c| {
            !matches!(
                c,
                std::path::Component::RootDir | std::path::Component::Normal(_)
            )
        })
    {
        return Err(ExactDeleteError::Conflict(
            "an exact absolute transcript path is required",
        ));
    }
    Ok(())
}
fn unavailable(id: &str, scope: &ExactTranscriptScope, reason: &str) -> ExactTranscriptContext {
    ExactTranscriptContext {
        schema: SCHEMA.into(),
        id: id.into(),
        workspace_id: scope.workspace_id.clone(),
        host_id: "local".into(),
        path: scope.path.clone(),
        state: "unavailable".into(),
        reason: Some(reason.into()),
        source_kind: None,
        archived: None,
        revision: None,
        observed_at: chrono::Utc::now().to_rfc3339(),
        effects: vec![],
        coordination: COORDINATION.into(),
        limits: ExactTranscriptLimits::default(),
    }
}
fn bounded(context: ExactTranscriptContext) -> Result<ExactTranscriptContext> {
    if context.effects.len() > context.limits.effect_entries
        || serde_json::to_vec(&context)
            .map_err(|_| ExactDeleteError::Failed("context serialization failed"))?
            .len()
            > context.limits.context_bytes
    {
        Err(ExactDeleteError::Unavailable(
            "exact deletion context exceeds its display limit",
        ))
    } else {
        Ok(context)
    }
}

#[cfg(unix)]
mod unix {
    use super::*;
    use rupu_runtime::standalone_run_metadata::StandaloneRunMetadata;
    use rustix::fs::{AtFlags, Mode, OFlags};
    use serde::Serialize;
    use sha2::{Digest, Sha256};
    use std::{
        fs::File,
        io::{Read, Seek, SeekFrom},
        os::unix::fs::MetadataExt,
        path::{Component, PathBuf},
    };

    const META_BYTES: usize = 262_144;
    const PROLOGUE_BYTES: usize = 262_144;
    const REGISTRY_BYTES: usize = 262_144;
    const MAX_ENTRIES: usize = 32;
    const DIR_FLAGS: OFlags = OFlags::RDONLY
        .union(OFlags::DIRECTORY)
        .union(OFlags::NOFOLLOW)
        .union(OFlags::NONBLOCK)
        .union(OFlags::CLOEXEC);
    const FILE_FLAGS: OFlags = OFlags::RDONLY
        .union(OFlags::NOFOLLOW)
        .union(OFlags::NONBLOCK)
        .union(OFlags::CLOEXEC);

    #[derive(Clone, Debug, PartialEq, Eq, Serialize)]
    struct FileIdentity {
        dev: u64,
        ino: u64,
        size: u64,
        mode: u32,
        uid: u32,
        nlink: u64,
        mtime: i64,
        mtime_ns: i64,
        ctime: i64,
        ctime_ns: i64,
    }
    impl FileIdentity {
        fn from(file: &File) -> Result<Self> {
            let m = file
                .metadata()
                .map_err(|_| ExactDeleteError::Unavailable("cannot inspect selected file"))?;
            Ok(Self {
                dev: m.dev(),
                ino: m.ino(),
                size: m.len(),
                mode: m.mode(),
                uid: m.uid(),
                nlink: m.nlink(),
                mtime: m.mtime(),
                mtime_ns: m.mtime_nsec(),
                ctime: m.ctime(),
                ctime_ns: m.ctime_nsec(),
            })
        }
    }
    #[derive(Clone, Debug, PartialEq, Eq, Serialize)]
    struct DirectoryIdentity {
        dev: u64,
        ino: u64,
        mode: u32,
        uid: u32,
    }
    impl DirectoryIdentity {
        fn from(file: &File) -> Result<Self> {
            let m = file
                .metadata()
                .map_err(|_| ExactDeleteError::Unavailable("cannot inspect selected directory"))?;
            if !m.is_dir() {
                return Err(ExactDeleteError::Unavailable(
                    "selected parent is not a directory",
                ));
            }
            Ok(Self {
                dev: m.dev(),
                ino: m.ino(),
                mode: m.mode(),
                uid: m.uid(),
            })
        }
    }
    struct Directory {
        path: PathBuf,
        file: File,
        identity: DirectoryIdentity,
    }
    impl Directory {
        fn open(path: &Path) -> Result<Self> {
            let file = open_directory(path)?;
            let identity = DirectoryIdentity::from(&file)?;
            Ok(Self {
                path: path.into(),
                file,
                identity,
            })
        }
        fn recheck(&self) -> Result<()> {
            let current = open_directory(&self.path)
                .map_err(|_| ExactDeleteError::Conflict("selected directory changed"))?;
            if DirectoryIdentity::from(&current)? != self.identity
                || DirectoryIdentity::from(&self.file)? != self.identity
            {
                return Err(ExactDeleteError::Conflict(
                    "selected directory identity changed",
                ));
            }
            Ok(())
        }
    }
    struct PinnedFile {
        file: File,
        path: PathBuf,
        name: String,
        identity: FileIdentity,
    }
    impl PinnedFile {
        fn open(parent: &Directory, name: &str) -> Result<Self> {
            let file = match rustix::fs::openat(&parent.file, name, FILE_FLAGS, Mode::empty()) {
                Ok(fd) => File::from(fd),
                Err(rustix::io::Errno::NOENT) => {
                    return Err(ExactDeleteError::NotFound("selected file is absent"))
                }
                Err(_) => {
                    return Err(ExactDeleteError::Unavailable(
                        "selected file cannot be opened without following links",
                    ))
                }
            };
            regular(&file)?;
            let identity = FileIdentity::from(&file)?;
            let captured = Self {
                file,
                path: parent.path.join(name),
                name: name.into(),
                identity,
            };
            captured.recheck(parent)?;
            Ok(captured)
        }
        fn recheck(&self, parent: &Directory) -> Result<()> {
            parent.recheck()?;
            regular(&self.file)?;
            let named = File::from(
                rustix::fs::openat(&parent.file, self.name.as_str(), FILE_FLAGS, Mode::empty())
                    .map_err(|_| ExactDeleteError::Conflict("selected file pathname changed"))?,
            );
            regular(&named)?;
            if FileIdentity::from(&self.file)? != self.identity
                || FileIdentity::from(&named)? != self.identity
            {
                return Err(ExactDeleteError::Conflict("selected file revision changed"));
            }
            Ok(())
        }
        fn read_bounded(&mut self, limit: usize, complete: bool) -> Result<Vec<u8>> {
            if complete && self.identity.size > limit as u64 {
                return Err(ExactDeleteError::Unavailable(
                    "metadata exceeds its read limit",
                ));
            }
            self.file
                .seek(SeekFrom::Start(0))
                .map_err(|_| ExactDeleteError::Unavailable("selected file cannot be read"))?;
            let mut data = Vec::new();
            (&mut self.file)
                .take(limit as u64 + 1)
                .read_to_end(&mut data)
                .map_err(|_| ExactDeleteError::Unavailable("selected file cannot be read"))?;
            if complete && data.len() > limit {
                return Err(ExactDeleteError::Unavailable(
                    "metadata exceeds its read limit",
                ));
            }
            if FileIdentity::from(&self.file)? != self.identity {
                return Err(ExactDeleteError::Conflict(
                    "selected file changed during capture",
                ));
            }
            Ok(data)
        }
    }
    #[derive(Clone, Debug, Serialize)]
    struct RunDirectoryObservation {
        path: String,
        state: &'static str,
        directory: Option<DirectoryIdentity>,
        run_record: Option<FileIdentity>,
    }

    pub(super) struct Snapshot {
        _lease: rupu_runtime::retention_lease::RetentionLease,
        global: Directory,
        workspace: Directory,
        registered_workspace: Directory,
        registry_parent: Directory,
        registry: PinnedFile,
        parent: Directory,
        transcript: PinnedFile,
        metadata: PinnedFile,
        metadata_value: StandaloneRunMetadata,
        registry_hash: String,
        metadata_hash: String,
        prologue_hash: String,
        run_directories: Vec<RunDirectoryObservation>,
        source_kind: &'static str,
        archived: bool,
        revision: String,
    }
    impl Snapshot {
        pub(super) fn context(
            &self,
            id: &str,
            scope: &ExactTranscriptScope,
        ) -> Result<ExactTranscriptContext> {
            let mut context = unavailable(id, scope, "");
            context.state = "ready".into();
            context.reason = None;
            context.source_kind = Some(self.source_kind.into());
            context.archived = Some(self.archived);
            context.revision = Some(self.revision.clone());
            context.effects = vec![
                ExactTranscriptEffect {
                    kind: "transcript".into(),
                    path: self.transcript.path.to_string_lossy().into_owned(),
                    bytes: self.transcript.identity.size.to_string(),
                },
                ExactTranscriptEffect {
                    kind: "metadata".into(),
                    path: self.metadata.path.to_string_lossy().into_owned(),
                    bytes: self.metadata.identity.size.to_string(),
                },
            ];
            bounded(context)
        }
        fn revision(&self, id: &str, scope: &ExactTranscriptScope) -> Result<String> {
            let bytes=serde_json::to_vec(&serde_json::json!({"schema":SCHEMA,"workspace_id":scope.workspace_id,"id":id,"path":scope.path,
                "global_path":self.global.path,"global":self.global.identity,"workspace_path":self.workspace.path,"workspace":self.workspace.identity,
                "registry_path":self.registry.path,"registry":self.registry.identity,"registry_hash":self.registry_hash,"parent":self.parent.identity,
                "transcript":self.transcript.identity,"prologue_hash":self.prologue_hash,"metadata":self.metadata.identity,"metadata_hash":self.metadata_hash,
                "source_kind":self.source_kind,"archived":self.archived,"run_directories":self.run_directories})).map_err(|_|ExactDeleteError::Failed("revision serialization failed"))?;
            Ok(format!("sha256:{}", hash(&bytes)))
        }
        fn recheck(&self, id: &str, scope: &ExactTranscriptScope) -> Result<()> {
            self.global.recheck()?;
            self.workspace.recheck()?;
            self.registered_workspace.recheck()?;
            self.registry.recheck(&self.registry_parent)?;
            self.transcript.recheck(&self.parent)?;
            self.metadata.recheck(&self.parent)?;
            guards(
                id,
                &scope.workspace_id,
                &self.workspace.path,
                &self.metadata_value,
            )?;
            let current = observe_run_directories(&self.global, id)?;
            if serde_json::to_vec(&current).ok() != serde_json::to_vec(&self.run_directories).ok() {
                return Err(ExactDeleteError::Conflict(
                    "run directory cleanup disposition changed",
                ));
            }
            Ok(())
        }
    }

    pub(super) fn capture_with_lease(
        global: &Path,
        id: &str,
        scope: &ExactTranscriptScope,
    ) -> Result<Snapshot> {
        // Resolve the configured alias only once. Pin that physical root before
        // acquiring its lease, then verify the same root after acquisition.
        let canonical_global = global
            .canonicalize()
            .map_err(|_| ExactDeleteError::Unavailable("owning global root is unavailable"))?;
        let global = Directory::open(&canonical_global)?;
        let lease = rupu_runtime::retention_lease::RetentionLease::try_acquire(&canonical_global)
            .map_err(|e| {
            if e.kind() == std::io::ErrorKind::WouldBlock {
                ExactDeleteError::Unavailable(
                    "another lifecycle operation holds the retention lease",
                )
            } else {
                ExactDeleteError::Unavailable("retention lifecycle lease is unavailable")
            }
        })?;
        global.recheck()?;
        let registry_parent = Directory::open(&canonical_global.join("workspaces"))?;
        let mut registry =
            PinnedFile::open(&registry_parent, &format!("{}.toml", scope.workspace_id))?;
        let registry_bytes = registry.read_bounded(REGISTRY_BYTES, true)?;
        let record: rupu_workspace::Workspace =
            toml::from_str(std::str::from_utf8(&registry_bytes).map_err(|_| {
                ExactDeleteError::Unavailable("workspace registration is malformed")
            })?)
            .map_err(|_| ExactDeleteError::Unavailable("workspace registration is malformed"))?;
        if record.id != scope.workspace_id {
            return Err(ExactDeleteError::Unavailable(
                "workspace registration identity does not match",
            ));
        }
        let registered = Path::new(&record.path);
        if !registered.is_absolute() {
            return Err(ExactDeleteError::Unavailable(
                "registered workspace root is not absolute",
            ));
        }
        // This opt-in path conservatively refuses registered symlink aliases,
        // including intermediate components. Retain the original spelling so
        // replacing it cannot silently preserve authority over its old target.
        let registered_workspace = Directory::open(registered)?;
        let canonical_workspace = registered.canonicalize().map_err(|_| {
            ExactDeleteError::Unavailable("registered workspace root is unavailable")
        })?;
        let workspace = Directory::open(&canonical_workspace)?;
        registered_workspace.recheck()?;
        if registered_workspace.identity != workspace.identity {
            return Err(ExactDeleteError::Conflict(
                "registered workspace root changed",
            ));
        }
        let source = Path::new(&scope.path);
        let filename = format!("{id}.jsonl");
        let sources = [
            (
                canonical_workspace.join(".rupu/transcripts"),
                "project_active",
                false,
            ),
            (
                canonical_workspace.join(".rupu/transcripts/archive"),
                "project_archive",
                true,
            ),
            (canonical_global.join("transcripts"), "global_active", false),
            (
                canonical_global.join("transcripts/archive"),
                "global_archive",
                true,
            ),
        ];
        let matches: Vec<_> = sources
            .into_iter()
            .filter(|(root, _, _)| root.join(&filename).as_os_str() == source.as_os_str())
            .collect();
        if matches.len() != 1 {
            return Err(ExactDeleteError::Unavailable(
                "path is not one exact transcript location for this registered workspace",
            ));
        }
        let (parent_path, source_kind, archived) = matches.into_iter().next().expect("one source");
        let parent = Directory::open(&parent_path)?;
        let mut transcript = PinnedFile::open(&parent, &filename)?;
        let mut metadata =
            PinnedFile::open(&parent, &format!("{id}.meta.json")).map_err(|e| match e {
                ExactDeleteError::NotFound(_) => {
                    ExactDeleteError::Unavailable("standalone ownership metadata is missing")
                }
                other => other,
            })?;
        let metadata_bytes = metadata.read_bounded(META_BYTES, true)?;
        let metadata_value: StandaloneRunMetadata = serde_json::from_slice(&metadata_bytes)
            .map_err(|_| ExactDeleteError::Unavailable("standalone metadata is malformed"))?;
        guards(
            id,
            &scope.workspace_id,
            &canonical_workspace,
            &metadata_value,
        )?;
        let prologue = transcript.read_bounded(PROLOGUE_BYTES, false)?;
        validate_prologue(&prologue, id, &scope.workspace_id)?;
        let run_directories = observe_run_directories(&global, id)?;
        let mut snapshot = Snapshot {
            _lease: lease,
            global,
            workspace,
            registered_workspace,
            registry_parent,
            registry,
            parent,
            transcript,
            metadata,
            metadata_value,
            registry_hash: hash(&registry_bytes),
            metadata_hash: hash(&metadata_bytes),
            prologue_hash: hash(&prologue[..prologue.len().min(PROLOGUE_BYTES)]),
            run_directories,
            source_kind,
            archived,
            revision: String::new(),
        };
        snapshot.recheck(id, scope)?;
        snapshot.revision = snapshot.revision(id, scope)?;
        Ok(snapshot)
    }
    pub(super) fn delete(
        global: &Path,
        id: &str,
        scope: &ExactTranscriptScope,
        revision: &str,
    ) -> Result<()> {
        let snapshot = capture_with_lease(global, id, scope)?;
        if snapshot.revision != revision {
            return Err(ExactDeleteError::Conflict(
                "reviewed transcript revision changed",
            ));
        }
        // All ownership, file, cleanup and liveness preconditions are checked
        // before the first mutation. This cannot make POSIX unlink conditional
        // against a writer that ignores the advisory lease.
        snapshot.recheck(id, scope)?;
        rustix::fs::unlinkat(
            &snapshot.parent.file,
            snapshot.transcript.name.as_str(),
            AtFlags::empty(),
        )
        .map_err(|_| {
            ExactDeleteError::Failed(
                "transcript unlink failed; refresh exact context before any further action",
            )
        })?;
        // Keep the captured ownership record until after the transcript unlink.
        // Refuse a replacement metadata inode instead of deleting it silently.
        if snapshot.global.recheck().is_err()
            || snapshot.workspace.recheck().is_err()
            || snapshot.registered_workspace.recheck().is_err()
            || snapshot
                .registry
                .recheck(&snapshot.registry_parent)
                .is_err()
            || snapshot.metadata.recheck(&snapshot.parent).is_err()
            || guards(
                id,
                &scope.workspace_id,
                &snapshot.workspace.path,
                &snapshot.metadata_value,
            )
            .is_err()
        {
            return Err(ExactDeleteError::Failed(
                "transcript removed but metadata changed; deletion is partial",
            ));
        }
        rustix::fs::unlinkat(
            &snapshot.parent.file,
            snapshot.metadata.name.as_str(),
            AtFlags::empty(),
        )
        .map_err(|_| {
            ExactDeleteError::Failed(
                "transcript removed but metadata unlink failed; deletion is partial",
            )
        })?;
        snapshot.parent.file.sync_all().map_err(|_| {
            ExactDeleteError::Failed(
                "selected files were unlinked but directory durability is unconfirmed",
            )
        })?;
        Ok(())
    }
    fn guards(
        id: &str,
        _workspace_id: &str,
        workspace: &Path,
        metadata: &StandaloneRunMetadata,
    ) -> Result<()> {
        if metadata.version != StandaloneRunMetadata::VERSION || metadata.run_id != id {
            return Err(ExactDeleteError::Unavailable(
                "standalone metadata version or run identity is unsupported",
            ));
        }
        super::super::ensure_standalone_transcript(id, Some(metadata))
            .map_err(|_| ExactDeleteError::Unavailable("transcript is managed by a session"))?;
        if metadata.backend_id != "local_checkout" {
            return Err(ExactDeleteError::Unavailable(
                "recorded process owner is not a supported local checkout",
            ));
        }
        if !metadata.workspace_path.is_absolute() {
            return Err(ExactDeleteError::Unavailable(
                "recorded workspace ownership is not absolute",
            ));
        }
        let recorded = metadata.workspace_path.canonicalize().map_err(|_| {
            ExactDeleteError::Unavailable("recorded workspace ownership cannot be verified")
        })?;
        if recorded != workspace {
            return Err(ExactDeleteError::Unavailable(
                "recorded workspace ownership does not match the selected registration",
            ));
        }
        let Some(pid) = metadata.pid.filter(|p| *p > 0 && *p <= i32::MAX as u32) else {
            return Err(ExactDeleteError::Unavailable(
                "recorded process liveness is unknown",
            ));
        };
        let pid = rustix::process::Pid::from_raw(pid as i32).ok_or(
            ExactDeleteError::Unavailable("recorded process liveness is unknown"),
        )?;
        require_absent_process(rustix::process::test_kill_process(pid))?;
        super::super::ensure_standalone_not_running(id, "delete", Some(metadata))
            .map_err(|_| ExactDeleteError::Unavailable("owning process is still alive"))?;
        Ok(())
    }
    fn require_absent_process(result: rustix::io::Result<()>) -> Result<()> {
        match result {
            Err(rustix::io::Errno::SRCH) => Ok(()),
            Ok(()) | Err(rustix::io::Errno::PERM) => Err(ExactDeleteError::Unavailable(
                "owning process is still alive or inaccessible",
            )),
            Err(_) => Err(ExactDeleteError::Unavailable(
                "recorded process liveness is unknown",
            )),
        }
    }
    fn validate_prologue(bytes: &[u8], id: &str, workspace: &str) -> Result<()> {
        let window = &bytes[..bytes.len().min(PROLOGUE_BYTES)];
        let mut consumed = 0;
        for line in window.split_inclusive(|b| *b == b'\n') {
            consumed += line.len();
            if !line.ends_with(b"\n") && (bytes.len() > PROLOGUE_BYTES || consumed < bytes.len()) {
                return Err(ExactDeleteError::Unavailable(
                    "RunStart exceeds the bounded prologue",
                ));
            }
            if line.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            let event: rupu_transcript::Event = serde_json::from_slice(line)
                .map_err(|_| ExactDeleteError::Unavailable("transcript prologue is malformed"))?;
            // Event ignores additive raw fields. Explicit remote authority or a
            // contradictory flat identity must not be erased by that projection.
            let raw: serde_json::Value = serde_json::from_slice(line)
                .map_err(|_| ExactDeleteError::Unavailable("transcript prologue is malformed"))?;
            if [
                &raw["host"],
                &raw["host_id"],
                &raw["data"]["host"],
                &raw["data"]["host_id"],
            ]
            .iter()
            .any(|value| !value.is_null() && value.as_str() != Some("local"))
                || (!raw["run_id"].is_null() && raw["run_id"].as_str() != Some(id))
                || (!raw["workspace_id"].is_null()
                    && raw["workspace_id"].as_str() != Some(workspace))
            {
                return Err(ExactDeleteError::Unavailable(
                    "RunStart contains contradictory or nonlocal ownership",
                ));
            }
            match event {
                rupu_transcript::Event::RunStart {
                    run_id,
                    workspace_id,
                    schema,
                    ..
                } if run_id == id
                    && workspace_id == workspace
                    && schema.is_none_or(|s| s == 1 || s == 2) =>
                {
                    return Ok(())
                }
                rupu_transcript::Event::RunStart { .. } => {
                    return Err(ExactDeleteError::Unavailable(
                        "RunStart does not prove the selected run and workspace",
                    ))
                }
                _ => {
                    return Err(ExactDeleteError::Unavailable(
                        "first transcript record is not a supported RunStart",
                    ))
                }
            }
        }
        Err(ExactDeleteError::Unavailable(
            "no bounded RunStart ownership record",
        ))
    }
    fn observe_run_directories(
        global: &Directory,
        id: &str,
    ) -> Result<Vec<RunDirectoryObservation>> {
        let mut result = Vec::new();
        for collection in ["runs", "runs-archive"] {
            let root_path = global.path.join(collection);
            let root = match open_directory_optional(&root_path)? {
                Some(file) => file,
                None => {
                    result.push(RunDirectoryObservation {
                        path: root_path.join(id).to_string_lossy().into_owned(),
                        state: "absent",
                        directory: None,
                        run_record: None,
                    });
                    continue;
                }
            };
            let file = match rustix::fs::openat(&root, id, DIR_FLAGS, Mode::empty()) {
                Ok(fd) => File::from(fd),
                Err(rustix::io::Errno::NOENT) => {
                    result.push(RunDirectoryObservation {
                        path: root_path.join(id).to_string_lossy().into_owned(),
                        state: "absent",
                        directory: None,
                        run_record: None,
                    });
                    continue;
                }
                Err(_) => {
                    return Err(ExactDeleteError::Unavailable(
                        "same-ID run directory is not safely inspectable",
                    ))
                }
            };
            let identity = DirectoryIdentity::from(&file)?;
            match rustix::fs::openat(&file, "run.json", FILE_FLAGS, Mode::empty()) {
                Ok(fd) => {
                    let record = File::from(fd);
                    regular(&record)?;
                    result.push(RunDirectoryObservation {
                        path: root_path.join(id).to_string_lossy().into_owned(),
                        state: "managed_run_preserved",
                        directory: Some(identity),
                        run_record: Some(FileIdentity::from(&record)?),
                    });
                }
                Err(rustix::io::Errno::NOENT) => {
                    // Begin lines in these global streams carry only run_id,
                    // never registered workspace authority. Recognize only the
                    // known flat stream names, but do NOT delete them on an ID
                    // coincidence. No recursive directory deletion is allowed.
                    let names = directory_names(&file)?;
                    for name in names {
                        if ![
                            "coverage.jsonl",
                            "coverage.jsonl.lock",
                            "netflow.jsonl",
                            "netflow.jsonl.lock",
                        ]
                        .contains(&name.as_str())
                        {
                            return Err(ExactDeleteError::Unavailable(
                                "same-ID stream directory contains unsupported entries",
                            ));
                        }
                        let entry = File::from(
                            rustix::fs::openat(&file, name.as_str(), FILE_FLAGS, Mode::empty())
                                .map_err(|_| {
                                    ExactDeleteError::Unavailable("same-ID stream entry is unsafe")
                                })?,
                        );
                        regular(&entry)?;
                    }
                    return Err(ExactDeleteError::Unavailable(
                        "global stream-only cleanup ownership is unproven; no files can be deleted",
                    ));
                }
                Err(_) => {
                    return Err(ExactDeleteError::Unavailable(
                        "same-ID managed run record is unsafe",
                    ))
                }
            }
        }
        Ok(result)
    }
    fn directory_names(file: &File) -> Result<Vec<String>> {
        let mut names = Vec::new();
        for entry in rustix::fs::Dir::read_from(file)
            .map_err(|_| ExactDeleteError::Unavailable("cannot inspect run directory"))?
        {
            let entry =
                entry.map_err(|_| ExactDeleteError::Unavailable("cannot inspect run directory"))?;
            let bytes = entry.file_name().to_bytes();
            if bytes == b"." || bytes == b".." {
                continue;
            }
            if names.len() >= MAX_ENTRIES {
                return Err(ExactDeleteError::Unavailable(
                    "same-ID run directory exceeds the entry bound",
                ));
            }
            names.push(String::from_utf8(bytes.to_vec()).map_err(|_| {
                ExactDeleteError::Unavailable("same-ID run directory has unsupported names")
            })?);
        }
        Ok(names)
    }
    fn open_directory_optional(path: &Path) -> Result<Option<File>> {
        match open_directory_io(path) {
            Ok(file) => Ok(Some(file)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(_) => Err(ExactDeleteError::Unavailable(
                "selected directory path contains unsupported links or entries",
            )),
        }
    }
    fn open_directory(path: &Path) -> Result<File> {
        open_directory_io(path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                ExactDeleteError::NotFound("selected directory is absent")
            } else {
                ExactDeleteError::Unavailable(
                    "selected directory cannot be opened without following links",
                )
            }
        })
    }
    fn open_directory_io(path: &Path) -> std::io::Result<File> {
        if !path.is_absolute() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "absolute path required",
            ));
        }
        let mut file = File::from(rustix::fs::open("/", DIR_FLAGS, Mode::empty())?);
        for part in path.components() {
            match part {
                Component::RootDir => (),
                Component::Normal(name) => {
                    file = File::from(rustix::fs::openat(&file, name, DIR_FLAGS, Mode::empty())?)
                }
                _ => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "normal path required",
                    ))
                }
            }
        }
        Ok(file)
    }
    fn regular(file: &File) -> Result<()> {
        let m = file
            .metadata()
            .map_err(|_| ExactDeleteError::Unavailable("cannot inspect source file"))?;
        if !m.is_file() || m.nlink() != 1 {
            return Err(ExactDeleteError::Unavailable(
                "source file must be regular and single-link",
            ));
        }
        Ok(())
    }
    fn hash(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    #[cfg(test)]
    mod tests {
        include!("transcript_exact_delete_tests.rs");
    }
}

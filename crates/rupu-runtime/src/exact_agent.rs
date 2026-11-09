//! Exact installed global agent bytes, independently bound execution workspace.
//! No directory scan, name fallback, staged source relocation or provider work.
use anyhow::{bail, ensure, Context, Result};
use rupu_agent::AgentSpec;
use rustix::fs::{Mode, OFlags};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::Read,
    os::unix::fs::MetadataExt,
    path::{Component, Path, PathBuf},
};

pub const FEATURE: &str = "agent.exact_global_run.v1";
pub const MAX_SOURCE: usize = 128 * 1024;
const MAX_REGISTRY: usize = 64 * 1024;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub file_key: String,
    pub definition_revision: String,
    pub effective_name: String,
    pub workspace_id: String,
    pub project_path: String,
    pub project_revision: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct Review {
    pub binding: Binding,
    pub definition_sha256: String,
    pub raw: String,
}
pub struct Loaded {
    pub review: Review,
    pub spec: AgentSpec,
    pub registration_raw: String,
}
#[derive(Deserialize)]
struct Registration {
    id: String,
    path: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct Stamp {
    dev: u64,
    ino: u64,
    len: u64,
    modified: (i64, i64),
    changed: (i64, i64),
    links: u64,
}
fn stamp(file: &File) -> Result<Stamp> {
    let m = file.metadata()?;
    Ok(Stamp {
        dev: m.dev(),
        ino: m.ino(),
        len: m.len(),
        modified: (m.mtime(), m.mtime_nsec()),
        changed: (m.ctime(), m.ctime_nsec()),
        links: m.nlink(),
    })
}
fn identity(file: &File) -> Result<(u64, u64)> {
    let s = stamp(file)?;
    Ok((s.dev, s.ino))
}
fn directory(path: &Path) -> Result<File> {
    ensure!(path.is_absolute(), "exact agent owner must be absolute");
    let flags =
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
    let mut fd = rustix::fs::open("/", flags, Mode::empty())?;
    for part in path.components() {
        match part {
            Component::RootDir => (),
            Component::Normal(p) => fd = rustix::fs::openat(&fd, p, flags, Mode::empty())?,
            _ => bail!("noncanonical owner components"),
        }
    }
    Ok(File::from(fd))
}
fn optional_layer(project: &Path) -> Result<Option<(u64, u64)>> {
    match std::fs::symlink_metadata(project.join(".rupu")) {
        Ok(_) => Ok(Some(identity(&directory(&project.join(".rupu"))?)?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}
fn regular(dir: &File, key: &str) -> Result<File> {
    let fd = rustix::fs::openat(
        dir,
        key,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    let file = File::from(fd);
    let meta = file.metadata()?;
    ensure!(
        meta.is_file() && meta.nlink() == 1,
        "exact source must be a single-link regular file"
    );
    Ok(file)
}
fn read(dir: &File, key: &str, cap: usize) -> Result<(Vec<u8>, Stamp)> {
    let mut file = regular(dir, key)?;
    let before = stamp(&file)?;
    ensure!(before.len <= cap as u64, "exact source byte limit exceeded");
    let mut bytes = Vec::new();
    (&mut file).take(cap as u64 + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= cap
            && bytes.len() as u64 == before.len
            && stamp(&file)? == before
            && stamp(&regular(dir, key)?)? == before,
        "exact source changed during read"
    );
    Ok((bytes, before))
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn revision(value: impl Serialize) -> Result<String> {
    Ok(digest(&serde_json::to_vec(&value)?))
}
fn safe_id(v: &str) -> bool {
    !v.is_empty()
        && v.len() <= 128
        && v.as_bytes()[0].is_ascii_alphanumeric()
        && v.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}
fn validate_key(key: &str) -> Result<()> {
    let stem = key
        .strip_suffix(".md")
        .context("exact global key must end in .md")?;
    ensure!(
        safe_id(stem),
        "exact global key must be one plain installed filename"
    );
    Ok(())
}
impl Binding {
    pub fn decode(raw: &str) -> Result<Self> {
        ensure!(raw.len() <= 16 * 1024, "exact binding exceeds limit");
        let value: Self = serde_json::from_str(raw)?;
        value.validate()?;
        Ok(value)
    }
    pub fn validate(&self) -> Result<()> {
        validate_key(&self.file_key)?;
        ensure!(
            safe_id(&self.workspace_id) && safe_id(&self.effective_name),
            "invalid exact binding identity"
        );
        for hash in [&self.definition_revision, &self.project_revision] {
            ensure!(
                hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()),
                "invalid exact binding revision"
            );
        }
        ensure!(
            self.project_path.len() <= 4096 && Path::new(&self.project_path).is_absolute(),
            "invalid exact project path"
        );
        Ok(())
    }
}
/// Reads exact global filename and exact registered project. Fingerprints include
/// source/registration bytes and anchored directory/file identities. A registration
/// changing only its last_run_at also makes a previous review stale, deliberately.
pub fn review(global: &Path, file_key: &str, workspace_id: &str) -> Result<Loaded> {
    validate_key(file_key)?;
    ensure!(safe_id(workspace_id), "invalid execution workspace ID");
    let global = global.canonicalize().context("global owner unavailable")?;
    let root = directory(&global)?;
    let agents = directory(&global.join("agents"))?;
    let registry = directory(&global.join("workspaces"))?;
    let (source, source_stamp) = read(&agents, file_key, MAX_SOURCE)?;
    let raw = String::from_utf8(source)?;
    let spec = AgentSpec::parse(&raw)?;
    ensure!(
        safe_id(&spec.name) && rupu_agent::builtin::find(&spec.name).is_none(),
        "reserved or unsupported effective agent name"
    );
    spec.validate_tools()?;
    let registry_key = format!("{workspace_id}.toml");
    let (record, record_stamp) = read(&registry, &registry_key, MAX_REGISTRY)?;
    let registration_raw = String::from_utf8(record)?;
    let registered: Registration = toml::from_str(&registration_raw)?;
    ensure!(
        registered.id == workspace_id && registered.path.len() <= 4096,
        "execution registration identity mismatch"
    );
    let project = PathBuf::from(&registered.path);
    let project_fd = directory(&project)?;
    ensure!(
        project.canonicalize()? == project,
        "execution registration must name its canonical root"
    );
    // Absence is explicit: the worker pins this project root, never ancestor discovery.
    let layer_id = optional_layer(&project)?;
    let registry_id = identity(&registry)?;
    let root_id = identity(&root)?;
    let agent_id = identity(&agents)?;
    let project_id = identity(&project_fd)?;
    let source_hash = digest(raw.as_bytes());
    let binding = Binding {
        file_key: file_key.into(),
        definition_revision: revision((
            &global,
            root_id,
            agent_id,
            file_key,
            &source_stamp,
            &source_hash,
        ))?,
        effective_name: spec.name.clone(),
        workspace_id: workspace_id.into(),
        project_path: registered.path,
        project_revision: revision((
            &global,
            root_id,
            registry_id,
            workspace_id,
            &record_stamp,
            digest(registration_raw.as_bytes()),
            project_id,
            layer_id,
        ))?,
    };
    ensure!(
        identity(&directory(&global.join("workspaces"))?)? == registry_id
            && identity(&directory(&global)?)? == root_id
            && identity(&directory(&global.join("agents"))?)? == agent_id
            && identity(&directory(&project)?)? == project_id
            && optional_layer(&project)? == layer_id,
        "exact owner directory changed"
    );
    ensure!(
        read(&agents, file_key, MAX_SOURCE)?.1 == source_stamp
            && read(&registry, &registry_key, MAX_REGISTRY)?.1 == record_stamp,
        "exact definition or project changed"
    );
    Ok(Loaded {
        review: Review {
            binding,
            definition_sha256: source_hash,
            raw,
        },
        spec,
        registration_raw,
    })
}
/// CP adapter and worker both use this check; a changed source never falls back
/// to a same-named project/global/builtin definition.
pub fn load(global: &Path, binding: &Binding, execution_dir: &Path) -> Result<Loaded> {
    binding.validate()?;
    let loaded = review(global, &binding.file_key, &binding.workspace_id)?;
    ensure!(
        loaded.review.binding == *binding,
        "exact definition or execution project revision changed"
    );
    ensure!(
        execution_dir.canonicalize()? == Path::new(&binding.project_path),
        "execution directory differs from reviewed project"
    );
    Ok(loaded)
}

#[cfg(test)]
mod tests;

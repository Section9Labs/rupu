//! Host-owned MCP credentials, isolated from provider accounts and RUPU_AUTH_FILE.
//! The caller supplies the trusted global root, never an arbitrary client path.
//! Registry lock precedes this store's lock for every registry-bound mutation.
use rupu_providers::private_file::{write_private_atomic, SidecarLock};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};

// Also serialize within the process on filesystems whose flock emulation is
// process-scoped. Cross-process exclusion remains the shared SidecarLock.
static WRITES: std::sync::Mutex<()> = std::sync::Mutex::new(());
const MAX_BYTES: u64 = 16 * 1024 * 1024;
const MAX_ENTRIES: usize = 4096;
const MAX_RECEIPTS: usize = 16384;
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid MCP credential request")]
    Invalid,
    #[error("MCP credential revision changed")]
    Conflict,
    #[error("MCP credential operation ID was already used; inspect its receipt")]
    OperationExists,
    #[error("MCP credential record not found")]
    NotFound,
    #[error("MCP credential store is unavailable")]
    Unavailable,
    #[error("MCP credential store is invalid or exceeds its limit")]
    InvalidStore,
    #[error("MCP credential store capacity reached")]
    Capacity,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub server_id: String,
    pub url: String,
    pub generation: String,
}
impl Binding {
    pub fn validate(&self) -> Result<(), Error> {
        let url = url::Url::parse(&self.url).map_err(|_| Error::Invalid)?;
        let loopback = match url.host() {
            Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
            Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
            _ => false,
        };
        if self.server_id.is_empty()
            || self.server_id.len() > 64
            || !self
                .server_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
            || !valid_id(&self.generation)
            || url.as_str() != self.url
            || url.host().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || !(url.scheme() == "https" || url.scheme() == "http" && loopback)
        {
            return Err(Error::Invalid);
        }
        Ok(())
    }
}
fn valid_id(id: &str) -> bool {
    id.len() == 26
        && id.as_bytes()[0] <= b'7'
        && id
            .bytes()
            .all(|b| b"0123456789ABCDEFGHJKMNPQRSTVWXYZ".contains(&b))
}
fn valid_revision(value: &str) -> bool {
    value == "absent"
        || value.len() == 32
            && value
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}
fn random_revision() -> String {
    rand::random::<[u8; 16]>()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Enroll,
    Remove,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub operation_id: String,
    pub action: Action,
    pub binding: Binding,
    pub previous_revision: String,
    pub credential_revision: String,
    pub present: bool,
}
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Status {
    pub binding: Binding,
    pub revision: String,
    pub present: bool,
}
// Deliberately no Debug or public serialization for secret-bearing records.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    binding: Binding,
    revision: String,
    token: Option<String>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    version: u32,
    entries: BTreeMap<String, Entry>,
    operations: BTreeMap<String, Receipt>,
}
impl Default for Document {
    fn default() -> Self {
        Self {
            version: 1,
            entries: BTreeMap::new(),
            operations: BTreeMap::new(),
        }
    }
}
#[derive(Clone)]
pub struct Store {
    path: PathBuf,
}
impl Store {
    pub fn for_global(global: &Path) -> Self {
        Self {
            path: global.join("mcp-credentials.json"),
        }
    }
    fn ordinary(path: &Path) -> Result<(), Error> {
        match fs::symlink_metadata(path) {
            Ok(meta) if meta.is_file() => {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if meta.permissions().mode() & 0o077 != 0 {
                        return Err(Error::Unavailable);
                    }
                }
                Ok(())
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            _ => Err(Error::Unavailable),
        }
    }
    fn read(&self) -> Result<Document, Error> {
        Self::ordinary(&self.path)?;
        let file = match fs::File::open(&self.path) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Document::default()),
            _ => return Err(Error::Unavailable),
        };
        let mut bytes = Vec::new();
        file.take(MAX_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| Error::Unavailable)?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err(Error::InvalidStore);
        }
        let document: Document = serde_json::from_slice(&bytes).map_err(|_| Error::InvalidStore)?;
        if document.version != 1
            || document.entries.len() > MAX_ENTRIES
            || document.operations.len() > MAX_RECEIPTS
        {
            return Err(Error::InvalidStore);
        }
        for (key, entry) in &document.entries {
            entry.binding.validate().map_err(|_| Error::InvalidStore)?;
            if key != &entry.binding.generation
                || !valid_revision(&entry.revision)
                || entry.revision == "absent"
                || entry.token.as_deref().is_some_and(|t| !valid_token(t))
            {
                return Err(Error::InvalidStore);
            }
        }
        for (key, receipt) in &document.operations {
            receipt
                .binding
                .validate()
                .map_err(|_| Error::InvalidStore)?;
            if key != &receipt.operation_id
                || !valid_id(key)
                || !valid_revision(&receipt.previous_revision)
                || !valid_revision(&receipt.credential_revision)
                || receipt.credential_revision == "absent"
                || receipt.present != (receipt.action == Action::Enroll)
            {
                return Err(Error::InvalidStore);
            }
        }
        Ok(document)
    }
    pub fn status(&self, binding: &Binding) -> Result<Status, Error> {
        binding.validate()?;
        let document = self.read()?;
        match document.entries.get(&binding.generation) {
            Some(entry) if &entry.binding != binding => Err(Error::Conflict),
            Some(entry) => Ok(Status {
                binding: binding.clone(),
                revision: entry.revision.clone(),
                present: entry.token.is_some(),
            }),
            None => Ok(Status {
                binding: binding.clone(),
                revision: "absent".into(),
                present: false,
            }),
        }
    }
    pub fn statuses(&self) -> Result<Vec<Status>, Error> {
        Ok(self
            .read()?
            .entries
            .into_values()
            .map(|e| Status {
                binding: e.binding,
                revision: e.revision,
                present: e.token.is_some(),
            })
            .collect())
    }
    pub fn receipt(&self, operation: &str) -> Result<Receipt, Error> {
        if !valid_id(operation) {
            return Err(Error::Invalid);
        }
        self.read()?
            .operations
            .remove(operation)
            .ok_or(Error::NotFound)
    }
    pub fn resolve(&self, binding: &Binding) -> Result<String, Error> {
        binding.validate()?;
        let document = self.read()?;
        let entry = document
            .entries
            .get(&binding.generation)
            .ok_or(Error::NotFound)?;
        if &entry.binding != binding {
            return Err(Error::Conflict);
        }
        entry.token.clone().ok_or(Error::NotFound)
    }
    /// The receipt and secret/tombstone share one atomic replacement. Returning
    /// an error after an uncertain filesystem outcome never authorizes a retry.
    pub fn mutate(
        &self,
        binding: &Binding,
        expected: &str,
        operation: &str,
        token: Option<&str>,
    ) -> Result<Receipt, Error> {
        binding.validate()?;
        if !valid_id(operation)
            || !valid_revision(expected)
            || token.is_some_and(|t| !valid_token(t))
        {
            return Err(Error::Invalid);
        }
        let _process = WRITES.lock().map_err(|_| Error::Unavailable)?;
        let parent = self.path.parent().ok_or(Error::Unavailable)?;
        fs::create_dir_all(parent).map_err(|_| Error::Unavailable)?;
        Self::ordinary(&self.path)?;
        Self::ordinary(&self.path.with_extension("json.lock"))?;
        let _lock = SidecarLock::acquire(&self.path, Duration::from_secs(2))
            .map_err(|_| Error::Unavailable)?;
        let mut document = self.read()?;
        if document.operations.contains_key(operation) {
            return Err(Error::OperationExists);
        }
        let current = document.entries.get(&binding.generation);
        if current.is_some_and(|e| &e.binding != binding)
            || current.map(|e| e.revision.as_str()).unwrap_or("absent") != expected
        {
            return Err(Error::Conflict);
        }
        if document.operations.len() >= MAX_RECEIPTS
            || current.is_none() && document.entries.len() >= MAX_ENTRIES
        {
            return Err(Error::Capacity);
        }
        let revision = random_revision();
        let receipt = Receipt {
            operation_id: operation.into(),
            action: if token.is_some() {
                Action::Enroll
            } else {
                Action::Remove
            },
            binding: binding.clone(),
            previous_revision: expected.into(),
            credential_revision: revision.clone(),
            present: token.is_some(),
        };
        document.entries.insert(
            binding.generation.clone(),
            Entry {
                binding: binding.clone(),
                revision,
                token: token.map(str::to_owned),
            },
        );
        document
            .operations
            .insert(operation.into(), receipt.clone());
        let bytes = serde_json::to_vec(&document).map_err(|_| Error::InvalidStore)?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err(Error::Capacity);
        }
        Self::ordinary(&self.path)?;
        write_private_atomic(&self.path, &bytes).map_err(|_| Error::Unavailable)?;
        // Persist the directory entry as well as the private-file bytes. An
        // error here is uncertain: the receipt may already be readable.
        #[cfg(unix)]
        fs::File::open(parent)
            .and_then(|dir| dir.sync_all())
            .map_err(|_| Error::Unavailable)?;
        Ok(receipt)
    }
}
fn valid_token(token: &str) -> bool {
    !token.is_empty() && token.len() <= 8192 && token.bytes().all(|b| (0x21..=0x7e).contains(&b))
}

#[cfg(test)]
mod tests;

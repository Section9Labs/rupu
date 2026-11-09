//! Known terminal receipts only. A page, its per-ID tombstones, the session
//! head, and a commit marker reach durable storage before active copies leave.
//! Interrupted publication reuses exact immutable bytes; lookup never scans
//! older pages. There is no lifetime receipt-count ceiling.
#![cfg(unix)]
use super::store::{self, Error, Record, Result};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::fs::File;

pub(super) const MAX_PAGE_RECORDS: usize = 20;
const MAX_PAGE_BYTES: u64 = 12 * 1024 * 1024;
const MAX_SMALL_BYTES: u64 = 16 * 1024;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ArchivePage {
    schema_version: u32,
    session_id: String,
    pub previous: Option<String>,
    pub records: Vec<Record>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Head {
    schema_version: u32,
    session_id: String,
    page: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Tombstone {
    schema_version: u32,
    approval_id: String,
    session_id: String,
    page: String,
    index: usize,
    record_fingerprint: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Commit {
    schema_version: u32,
    session_id: String,
    page: String,
}

pub(super) struct Archive {
    root: File,
    pages: File,
    ids: File,
    heads: File,
    commits: File,
}
impl Archive {
    pub fn open(parent: &File) -> Result<Self> {
        let root = store::private_directory(parent, "archive")?;
        Ok(Self {
            pages: store::private_directory(&root, "pages")?,
            ids: store::private_directory(&root, "ids")?,
            heads: store::private_directory(&root, "heads")?,
            commits: store::private_directory(&root, "commits")?,
            root,
        })
    }
    pub fn check_location(&self, parent: &File) -> Result<()> {
        store::same_entry(parent, "archive", &self.root)?;
        for (name, fd) in [
            ("pages", &self.pages),
            ("ids", &self.ids),
            ("heads", &self.heads),
            ("commits", &self.commits),
        ] {
            store::same_entry(&self.root, name, fd)?;
        }
        Ok(())
    }
    pub fn head(&self, session: &str) -> Result<Option<String>> {
        let Some(head) = self.raw_head(session)? else {
            return Ok(None);
        };
        if self.is_committed(&head, session)? {
            return Ok(Some(head));
        }
        // At most one head can be in publication: append resumes it before
        // starting another. Observers use its committed predecessor while its
        // exact records are still active, avoiding duplicate active/archive rows.
        let previous = self.page(&head, session)?.previous;
        if let Some(hash) = &previous {
            if !self.is_committed(hash, session)? {
                return Err(Error::Invalid);
            }
        }
        Ok(previous)
    }
    fn raw_head(&self, session: &str) -> Result<Option<String>> {
        store::valid_id(session)?;
        let head: Option<Head> =
            store::read_optional(&self.heads, &store::record_name(session), MAX_SMALL_BYTES)?;
        match head {
            None => Ok(None),
            Some(head) => {
                if head.schema_version != 1 || head.session_id != session || !valid_hash(&head.page)
                {
                    return Err(Error::Invalid);
                }
                Ok(Some(head.page))
            }
        }
    }
    fn is_committed(&self, hash: &str, session: &str) -> Result<bool> {
        let marker: Option<Commit> =
            store::read_optional(&self.commits, &format!("{hash}.json"), MAX_SMALL_BYTES)?;
        match marker {
            None => Ok(false),
            Some(marker) => {
                if marker.schema_version != 1 || marker.page != hash || marker.session_id != session
                {
                    return Err(Error::Invalid);
                }
                Ok(true)
            }
        }
    }
    pub fn page(&self, hash: &str, session: &str) -> Result<ArchivePage> {
        if !valid_hash(hash) {
            return Err(Error::Invalid);
        }
        let page: ArchivePage =
            store::read_optional(&self.pages, &format!("{hash}.json"), MAX_PAGE_BYTES)?
                .ok_or(Error::Invalid)?;
        if page.schema_version != 1
            || page.session_id != session
            || page.records.is_empty()
            || page.records.len() > MAX_PAGE_RECORDS
            || page.previous.as_ref().is_some_and(|v| !valid_hash(v))
            || store::bytes_hash(&store::encode(&page, MAX_PAGE_BYTES)?) != hash
        {
            return Err(Error::Invalid);
        }
        let mut ids = std::collections::BTreeSet::new();
        for row in &page.records {
            store::validate_record(row)?;
            if !row.state.is_known_terminal()
                || row.intent.session_id != session
                || !ids.insert(&row.intent.approval_id)
            {
                return Err(Error::Invalid);
            }
        }
        Ok(page)
    }
    fn tombstone(&self, id: &str) -> Result<Option<Tombstone>> {
        store::valid_id(id)?;
        let tomb: Option<Tombstone> =
            store::read_optional(&self.ids, &store::record_name(id), MAX_SMALL_BYTES)?;
        if let Some(t) = &tomb {
            if t.schema_version != 1
                || t.approval_id != id
                || !valid_hash(&t.page)
                || t.index >= MAX_PAGE_RECORDS
            {
                return Err(Error::Invalid);
            }
        }
        Ok(tomb)
    }
    fn lookup_tombstone(&self, t: &Tombstone) -> Result<Record> {
        let page = self.page(&t.page, &t.session_id)?;
        let row = page.records.get(t.index).ok_or(Error::Invalid)?;
        if row.intent.approval_id != t.approval_id
            || store::fingerprint(row)? != t.record_fingerprint
        {
            return Err(Error::Invalid);
        }
        Ok(row.clone())
    }
    pub fn lookup_any(&self, id: &str) -> Result<Option<Record>> {
        self.tombstone(id)?
            .map(|t| self.lookup_tombstone(&t))
            .transpose()
    }
    pub fn committed_lookup(&self, id: &str) -> Result<Option<Record>> {
        let Some(t) = self.tombstone(id)? else {
            return Ok(None);
        };
        let marker: Option<Commit> =
            store::read_optional(&self.commits, &format!("{}.json", t.page), MAX_SMALL_BYTES)?;
        let Some(marker) = marker else {
            return Ok(None);
        };
        if marker.schema_version != 1 || marker.page != t.page || marker.session_id != t.session_id
        {
            return Err(Error::Invalid);
        }
        self.lookup_tombstone(&t).map(Some)
    }
    /// Finish an interrupted page against the whole bounded active snapshot,
    /// before selecting any new batch. Newly terminal rows may sort ahead of
    /// the old page, so a fresh first-20 selection is not its recovery proof.
    pub fn resume_pending(&self, active: &[Record]) -> Result<Option<Vec<Record>>> {
        for row in active {
            let Some(tomb) = self.tombstone(&row.intent.approval_id)? else {
                continue;
            };
            if self.committed_lookup(&row.intent.approval_id)?.is_some() {
                continue;
            }
            let page = self.page(&tomb.page, &tomb.session_id)?;
            for old in &page.records {
                let current = active
                    .iter()
                    .find(|candidate| candidate.intent.approval_id == old.intent.approval_id)
                    .ok_or(Error::Invalid)?;
                if current != old {
                    return Err(Error::Invalid);
                }
            }
            self.commit_page(&tomb.page, &page)?;
            return Ok(Some(page.records));
        }
        Ok(None)
    }
    /// Returns the exact committed batch; callers may remove only these active
    /// rows after comparing their current complete record bytes.
    pub fn append(&self, rows: &[Record]) -> Result<Vec<Record>> {
        if rows.is_empty() || rows.len() > MAX_PAGE_RECORDS {
            return Err(Error::Invalid);
        }
        let session = &rows[0].intent.session_id;
        for row in rows {
            if !row.state.is_known_terminal() || &row.intent.session_id != session {
                return Err(Error::Invalid);
            }
            store::validate_record(row)?;
        }
        // If an interrupted attempt published a tombstone, finish that exact
        // page rather than inventing a different page or freeing the ID.
        if let Some(tomb) = rows
            .iter()
            .map(|r| self.tombstone(&r.intent.approval_id))
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .flatten()
            .next()
        {
            let page = self.page(&tomb.page, session)?;
            for row in &page.records {
                let selected = rows
                    .iter()
                    .find(|v| v.intent.approval_id == row.intent.approval_id)
                    .ok_or(Error::Invalid)?;
                if selected != row {
                    return Err(Error::Invalid);
                }
            }
            self.commit_page(&tomb.page, &page)?;
            return Ok(page.records);
        }
        let page = ArchivePage {
            schema_version: 1,
            session_id: session.clone(),
            previous: self.head(session)?,
            records: rows.to_vec(),
        };
        let bytes = store::encode(&page, MAX_PAGE_BYTES)?;
        let hash = store::bytes_hash(&bytes);
        publish_immutable(&self.pages, &format!("{hash}.json"), &page, MAX_PAGE_BYTES)?;
        self.commit_page(&hash, &page)?;
        Ok(page.records)
    }
    fn commit_page(&self, hash: &str, page: &ArchivePage) -> Result<()> {
        // Re-read and re-sync visible orphans before adopting them as durable.
        if self.page(hash, &page.session_id)? != *page {
            return Err(Error::Invalid);
        }
        self.pages.sync_all()?;
        for (index, row) in page.records.iter().enumerate() {
            let tomb = Tombstone {
                schema_version: 1,
                approval_id: row.intent.approval_id.clone(),
                session_id: page.session_id.clone(),
                page: hash.into(),
                index,
                record_fingerprint: store::fingerprint(row)?,
            };
            publish_immutable(
                &self.ids,
                &store::record_name(&row.intent.approval_id),
                &tomb,
                MAX_SMALL_BYTES,
            )?;
        }
        let current = self.raw_head(&page.session_id)?;
        if current.as_deref() != Some(hash) {
            if current != page.previous {
                return Err(Error::Conflict);
            }
            let head = Head {
                schema_version: 1,
                session_id: page.session_id.clone(),
                page: hash.into(),
            };
            store::publish(
                &self.heads,
                &store::record_name(&page.session_id),
                &head,
                current.is_some(),
                MAX_SMALL_BYTES,
            )?;
        } else {
            self.heads.sync_all()?;
        }
        let marker = Commit {
            schema_version: 1,
            session_id: page.session_id.clone(),
            page: hash.into(),
        };
        publish_immutable(
            &self.commits,
            &format!("{hash}.json"),
            &marker,
            MAX_SMALL_BYTES,
        )?;
        Ok(())
    }
}
fn publish_immutable<T: Serialize + serde::de::DeserializeOwned + PartialEq>(
    dir: &File,
    name: &str,
    value: &T,
    max: u64,
) -> Result<()> {
    if let Some(existing) = store::read_optional::<T>(dir, name, max)? {
        if existing != *value {
            return Err(Error::Conflict);
        }
        // Existing content is immutable and its file was synced before
        // publication. Sync directory to adopt a prior uncertain rename/link.
        dir.sync_all()?;
        Ok(())
    } else {
        store::publish(dir, name, value, false, max)
    }
}
fn valid_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Cursor {
    pub session_id: String,
    pub snapshot: String,
    pub active_offset: usize,
    pub archive_page: Option<String>,
    pub archive_offset: usize,
    pub limit: usize,
}
impl Cursor {
    pub fn encode(&self, key: &[u8; 32]) -> Result<String> {
        let bytes = store::encode(self, 4096)?;
        let mut mac = Hmac::<Sha256>::new_from_slice(key).map_err(|_| Error::Invalid)?;
        mac.update(b"rupu-session-approval-cursor-v1\0");
        mac.update(&bytes);
        Ok(format!(
            "{}.{}",
            hex::encode(bytes),
            hex::encode(mac.finalize().into_bytes())
        ))
    }
    pub fn decode(value: &str, key: &[u8; 32]) -> Result<Self> {
        if value.len() > 8192 {
            return Err(Error::Invalid);
        }
        let (body, signature) = value.split_once('.').ok_or(Error::Invalid)?;
        let bytes = hex::decode(body).map_err(|_| Error::Invalid)?;
        let signature = hex::decode(signature).map_err(|_| Error::Invalid)?;
        let mut mac = Hmac::<Sha256>::new_from_slice(key).map_err(|_| Error::Invalid)?;
        mac.update(b"rupu-session-approval-cursor-v1\0");
        mac.update(&bytes);
        mac.verify_slice(&signature).map_err(|_| Error::Invalid)?;
        let value: Self = serde_json::from_slice(&bytes).map_err(|_| Error::Invalid)?;
        store::valid_id(&value.session_id)?;
        if value.active_offset > store::MAX_ACTIVE_RECORDS
            || value.archive_offset > MAX_PAGE_RECORDS
            || value.archive_page.as_ref().is_some_and(|v| !valid_hash(v))
            || !(1..=50).contains(&value.limit)
        {
            return Err(Error::Invalid);
        }
        Ok(value)
    }
}

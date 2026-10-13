//! Shipping a placed unit's skills to the host it runs on (spec
//! `docs/superpowers/specs/2026-10-13-rupu-skills-on-placed-units-design.md`
//! §4): what a connector's [`HostConnector::ship_skills`] packs, what it
//! answers, and the per-host memo that makes a fan-out send each skill once.
//!
//! [`HostConnector::ship_skills`]: crate::host::connector::HostConnector::ship_skills

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use rupu_skills::ship::{
    pack_skill, PackedSkill, ShipError, SkillBundleManifest, MAX_BUNDLE_BYTES,
    MAX_SHIPPED_SKILL_BYTES,
};
use rupu_skills::tree::{stamp, walk_tree, EntryKind};
use rupu_skills::{Layer, Skill, Stamp};

use crate::host::connector::HostConnectorError;

/// What a unit needs shipped: its resolved skills, packed on demand.
#[derive(Clone, Default)]
pub struct SkillBundle {
    skills: Vec<Arc<Skill>>,
}

impl SkillBundle {
    pub fn new(skills: Vec<Arc<Skill>>) -> Self {
        Self { skills }
    }

    pub fn is_empty(&self) -> bool {
        self.skills.is_empty()
    }

    pub fn skills(&self) -> &[Arc<Skill>] {
        &self.skills
    }

    /// Packs every skill, refusing a skill over [`MAX_SHIPPED_SKILL_BYTES`]
    /// or a bundle over [`MAX_BUNDLE_BYTES`] before reading a byte. A pack is
    /// cached for the process until the skill's stat-only stamp changes.
    /// Blocking: call from `spawn_blocking`.
    pub fn pack(&self) -> Result<Vec<Arc<PackedSkill>>, ShipError> {
        let mut stamps = Vec::with_capacity(self.skills.len());
        let mut total = 0u64;
        for skill in &self.skills {
            let entries = walk_tree(&skill.root).map_err(|source| ShipError::Tree {
                name: skill.name.to_string(),
                source,
            })?;
            let bytes: u64 = entries
                .iter()
                .map(|e| match e.kind {
                    EntryKind::File { len, .. } => len,
                    EntryKind::Symlink { .. } => 0,
                })
                .sum();
            if bytes > MAX_SHIPPED_SKILL_BYTES {
                return Err(ShipError::TooLarge {
                    name: skill.name.to_string(),
                    bytes,
                    limit: MAX_SHIPPED_SKILL_BYTES,
                });
            }
            total += bytes;
            stamps.push(stamp(&entries));
        }
        if total > MAX_BUNDLE_BYTES {
            return Err(ShipError::BundleTooLarge {
                bytes: total,
                limit: MAX_BUNDLE_BYTES,
            });
        }
        let packed = self
            .skills
            .iter()
            .zip(stamps)
            .map(|(skill, stamp)| cached_pack(skill, stamp))
            .collect::<Result<Vec<_>, _>>()?;
        // The archives carry headers on top of the content, and the host caps
        // what it receives by archive bytes.
        let archived: u64 = packed.iter().map(|p| p.bytes.len() as u64).sum();
        if archived > MAX_BUNDLE_BYTES {
            return Err(ShipError::BundleTooLarge {
                bytes: archived,
                limit: MAX_BUNDLE_BYTES,
            });
        }
        Ok(packed)
    }
}

type PackKey = (PathBuf, Layer, String);
type PackCache = Mutex<HashMap<PackKey, (Stamp, Arc<PackedSkill>)>>;

/// One pack per skill, replaced when its stamp changes, so the cache holds
/// at most one archive per skill ever shipped.
fn pack_cache() -> &'static PackCache {
    static CACHE: OnceLock<PackCache> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

fn cached_pack(skill: &Skill, stamp: Stamp) -> Result<Arc<PackedSkill>, ShipError> {
    let key = (skill.root.clone(), skill.layer, skill.name.to_string());
    let cache = pack_cache();
    if let Some((s, p)) = cache.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        if *s == stamp {
            return Ok(Arc::clone(p));
        }
    }
    let packed = Arc::new(pack_skill(skill)?);
    cache
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(key, (stamp, Arc::clone(&packed)));
    Ok(packed)
}

/// [`SkillBundle::pack`] on the blocking pool; a failure names `who` (the
/// host the unit was bound for).
pub async fn pack_bundle(
    bundle: &SkillBundle,
    who: &str,
) -> Result<Vec<Arc<PackedSkill>>, HostConnectorError> {
    let bundle = bundle.clone();
    tokio::task::spawn_blocking(move || bundle.pack())
        .await
        .map_err(|e| HostConnectorError::Internal(format!("{who}: packing skills: {e}")))?
        .map_err(|e| HostConnectorError::Invalid(format!("{who}: {e}")))
}

/// The manifest a unit launches with: every packed skill, shipped now or
/// already on the host.
pub fn manifest_of(packed: &[Arc<PackedSkill>]) -> SkillBundleManifest {
    SkillBundleManifest {
        version: SkillBundleManifest::VERSION,
        skills: packed.iter().map(|p| p.reference()).collect(),
    }
}

/// What [`HostConnector::ship_skills`] did.
///
/// [`HostConnector::ship_skills`]: crate::host::connector::HostConnector::ship_skills
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShipOutcome {
    /// The host has every skill; launch with this manifest.
    Shipped(SkillBundleManifest),
    /// Nothing to ship: no skills, or the unit runs where they are installed.
    NotNeeded,
    /// The host can't take shipped skills; the unit runs with the skills it
    /// has installed. The text says why; the fleet dispatcher's warning adds
    /// that the unit uses the host's installed skills, the same for every
    /// transport.
    Unsupported(String),
}

/// What a connector's transfer did with the skills it was handed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Transfer {
    /// The host holds every one of them now.
    Done,
    /// The host can't take shipped skills; the text names it and why.
    Downgrade(String),
}

/// Per-connector memo: the skills this host confirmed it holds, whether it
/// said it can't take shipped skills at all, and a lock that makes
/// concurrent ships to the host wait on one transfer. A connector lives as
/// long as its registry entry, so a downgrade is asked once per host per
/// process, never once per unit.
#[derive(Default)]
pub struct ShipMemo {
    confirmed: Mutex<HashSet<(String, String)>>,
    downgrade: Mutex<Option<String>>,
    lock: tokio::sync::Mutex<()>,
}

fn memo_key(p: &PackedSkill) -> (String, String) {
    (p.hash.hex(), p.name.clone())
}

impl ShipMemo {
    /// Calls `transfer` with the skills not yet confirmed — never when there
    /// are none, or once the host answered [`Transfer::Downgrade`] — and
    /// remembers what it answered: `Done` confirms them, `Downgrade` holds
    /// for every later call. An error remembers nothing.
    pub async fn ship<F, Fut>(
        &self,
        packed: &[Arc<PackedSkill>],
        transfer: F,
    ) -> Result<Transfer, HostConnectorError>
    where
        F: FnOnce(Vec<Arc<PackedSkill>>) -> Fut,
        Fut: Future<Output = Result<Transfer, HostConnectorError>>,
    {
        let _held = self.lock.lock().await;
        if let Some(m) = self
            .downgrade
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
        {
            return Ok(Transfer::Downgrade(m));
        }
        let pending: Vec<Arc<PackedSkill>> = {
            let confirmed = self.confirmed.lock().unwrap_or_else(|e| e.into_inner());
            packed
                .iter()
                .filter(|p| !confirmed.contains(&memo_key(p)))
                .cloned()
                .collect()
        };
        if pending.is_empty() {
            return Ok(Transfer::Done);
        }
        let keys: Vec<_> = pending.iter().map(|p| memo_key(p)).collect();
        let done = transfer(pending).await?;
        match &done {
            Transfer::Done => self
                .confirmed
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .extend(keys),
            Transfer::Downgrade(m) => {
                *self.downgrade.lock().unwrap_or_else(|e| e.into_inner()) = Some(m.clone())
            }
        }
        Ok(done)
    }
}

/// The one `ship_skills` of a transport that ships: pack `bundle` (a failure
/// names `who`), then run `transfer` through `memo`. Each transport supplies
/// only its transfer.
pub async fn ship_through<F, Fut>(
    memo: &ShipMemo,
    bundle: &SkillBundle,
    who: &str,
    transfer: F,
) -> Result<ShipOutcome, HostConnectorError>
where
    F: FnOnce(Vec<Arc<PackedSkill>>) -> Fut,
    Fut: Future<Output = Result<Transfer, HostConnectorError>>,
{
    if bundle.is_empty() {
        return Ok(ShipOutcome::NotNeeded);
    }
    let packed = pack_bundle(bundle, who).await?;
    Ok(match memo.ship(&packed, transfer).await? {
        Transfer::Done => ShipOutcome::Shipped(manifest_of(&packed)),
        Transfer::Downgrade(m) => ShipOutcome::Unsupported(m),
    })
}

/// What a transport that can't ship skills yet (tunnel, bucket) answers.
pub(crate) fn not_built(host_id: &str) -> ShipOutcome {
    ShipOutcome::Unsupported(format!(
        "host {host_id}: shipping skills over this transport is not built yet"
    ))
}

/// A launch carrying a skill bundle to a transport that can't deliver one:
/// refused, so the unit never runs without the skills it was shipped.
pub(crate) fn refuse_bundle_launch(host_id: &str) -> HostConnectorError {
    HostConnectorError::Unsupported(format!(
        "host {host_id}: shipped skills (skill_bundle) can't be delivered over this transport \
         yet, so this launch is refused"
    ))
}

/// `e` with `who` (the host) in front of its message.
pub(crate) fn prefixed(e: HostConnectorError, who: &str) -> HostConnectorError {
    use HostConnectorError as E;
    match e {
        E::Unreachable(m) => E::Unreachable(format!("{who}: {m}")),
        E::NotFound(m) => E::NotFound(format!("{who}: {m}")),
        E::Remote(c, m) => E::Remote(c, format!("{who}: {m}")),
        E::NotJson(m) => E::NotJson(format!("{who}: {m}")),
        E::Invalid(m) => E::Invalid(format!("{who}: {m}")),
        E::Internal(m) => E::Internal(format!("{who}: {m}")),
        E::Unsupported(m) => E::Unsupported(format!("{who}: {m}")),
        E::Unauthorized => E::Unauthorized,
    }
}

/// The `hash:layer:name` form `rupu __skills missing` takes and prints.
pub(crate) fn ref_arg(p: &PackedSkill) -> String {
    format!("{}:{}:{}", p.hash.hex(), p.layer, p.name)
}

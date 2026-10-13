//! Installing skills pinned to a commit: the stock fleet's skills, which
//! `rupu fleet install` takes from the built-in profiles' `[bundle]` (spec
//! `docs/superpowers/specs/2026-10-13-rupu-fleet-skills-design.md` §5.2).
//! A pin installs through the same path and writes the same source record
//! as `rupu skill install <source> --ref <commit>`, so `rupu skill update`
//! knows where a fleet skill came from.

use crate::install::{
    find_skills, install_selected, record_path_of, Installed, Selection, Tracking,
};
use crate::record::{now_secs, read_record, write_record, SourceRecord, RECORD_VERSION};
use crate::skill::{load_skill, Layer};
use crate::source::{parse_source, Fetch, GitSource, Source};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pin {
    pub name: String,
    pub source: String,
    pub tag: String,
    pub commit: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeptReason {
    /// The installed copy no longer matches its record: edited locally.
    LocallyModified,
    /// No record, or a record naming another source: installed by hand.
    OtherSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PinOutcome {
    Created,
    Skipped,
    Updated { from: Option<String> },
    Replaced,
    Kept(KeptReason),
    Failed(String),
}

#[derive(Debug, Default)]
pub struct PinReport {
    pub results: Vec<(Pin, PinOutcome)>,
}

impl PinReport {
    /// The pins that did not end up installed.
    pub fn missing(&self) -> Vec<&Pin> {
        self.results
            .iter()
            .filter(|(_, o)| matches!(o, PinOutcome::Failed(_)))
            .map(|(p, _)| p)
            .collect()
    }
}

/// What to do with one pin, decided from disk before anything is fetched.
enum Plan {
    Install(PinOutcome),
    Done(PinOutcome),
}

pub fn install_pins(
    pins: &[Pin],
    skills_dir: &Path,
    layer: Layer,
    force: bool,
    fetcher: &dyn Fetch,
) -> PinReport {
    let mut outcomes: Vec<Option<PinOutcome>> = vec![None; pins.len()];
    let mut groups: BTreeMap<(String, String), Vec<usize>> = BTreeMap::new();
    for (i, pin) in pins.iter().enumerate() {
        let url = match git_url(pin) {
            Ok(u) => u,
            Err(e) => {
                outcomes[i] = Some(PinOutcome::Failed(e));
                continue;
            }
        };
        match plan(pin, &url, skills_dir, layer, force) {
            Plan::Done(o) => outcomes[i] = Some(o),
            Plan::Install(o) => {
                outcomes[i] = Some(o);
                groups.entry((url, pin.commit.clone())).or_default().push(i);
            }
        }
    }
    for ((url, commit), members) in groups {
        let src = GitSource {
            url: url.clone(),
            git_ref: Some(commit.clone()),
            path: None,
        };
        let tree = match fetcher.fetch(&src) {
            Ok(t) => t,
            Err(e) => {
                for i in members {
                    outcomes[i] = Some(PinOutcome::Failed(format!(
                        "fetching {url} at {commit}: {e}"
                    )));
                }
                continue;
            }
        };
        if tree.commit.as_deref() != Some(commit.as_str()) {
            let got = tree.commit.clone().unwrap_or_else(|| "no commit".into());
            for i in members {
                outcomes[i] = Some(PinOutcome::Failed(format!(
                    "fetched {got} from {url}, the fleet pins {commit}"
                )));
            }
            continue;
        }
        let found = match find_skills(&tree.root) {
            Ok(f) => f,
            Err(e) => {
                for i in members {
                    outcomes[i] = Some(PinOutcome::Failed(e.to_string()));
                }
                continue;
            }
        };
        for i in members {
            let pin = &pins[i];
            let mut record = |inst: &Installed| -> Result<Tracking, crate::install::InstallError> {
                let rec = SourceRecord {
                    version: RECORD_VERSION,
                    source: pin.source.clone(),
                    url: Some(url.clone()),
                    git_ref: Some(commit.clone()),
                    path: record_path_of(None, &inst.rel),
                    commit: tree.commit.clone(),
                    local: None,
                    content_hash: inst.content_hash.hex(),
                    installed_at: now_secs(),
                };
                write_record(skills_dir, inst.name.as_str(), &rec).map_err(|source| {
                    crate::install::InstallError::Io {
                        path: crate::record::record_path(skills_dir, inst.name.as_str()),
                        source,
                    }
                })?;
                Ok(Tracking::Recorded)
            };
            if let Err(e) = install_selected(
                &found,
                &Selection::Named(vec![pin.name.clone()]),
                skills_dir,
                layer,
                true,
                &mut record,
            ) {
                outcomes[i] = Some(PinOutcome::Failed(e.to_string()));
            }
        }
    }
    PinReport {
        results: pins
            .iter()
            .cloned()
            .zip(
                outcomes
                    .into_iter()
                    .map(|o| o.expect("every pin has an outcome")),
            )
            .collect(),
    }
}

/// The https URL a pin's source names (`owner/repo` or a URL).
fn git_url(pin: &Pin) -> Result<String, String> {
    match parse_source(&pin.source, Some(&pin.commit), None) {
        Ok(Source::Git(g)) if g.path.is_none() => Ok(g.url),
        Ok(Source::Git(_)) => Err(format!(
            "{}: a pinned source names a repository, not a path in it",
            pin.source
        )),
        Ok(Source::Local(_)) => Err(format!(
            "{}: a pinned source must be a git repository",
            pin.source
        )),
        Err(e) => Err(format!("{}: {e}", pin.source)),
    }
}

fn plan(pin: &Pin, url: &str, skills_dir: &Path, layer: Layer, force: bool) -> Plan {
    let dir = skills_dir.join(&pin.name);
    if !dir.join("SKILL.md").is_file() {
        return Plan::Install(PinOutcome::Created);
    }
    let kept = |reason| {
        if force {
            Plan::Install(PinOutcome::Replaced)
        } else {
            Plan::Done(PinOutcome::Kept(reason))
        }
    };
    let rec = match read_record(skills_dir, &pin.name) {
        Ok(Some(r)) if r.url.as_deref() == Some(url) => r,
        Ok(_) => return kept(KeptReason::OtherSource),
        Err(_) if force => return Plan::Install(PinOutcome::Replaced),
        Err(e) => return Plan::Done(PinOutcome::Failed(e.to_string())),
    };
    let current = load_skill(&dir, layer)
        .map_err(|e| e.to_string())
        .and_then(|s| s.content_hash().map_err(|e| e.to_string()));
    let current = match current {
        Ok(h) => h.hex(),
        Err(_) if force => return Plan::Install(PinOutcome::Replaced),
        Err(e) => return Plan::Done(PinOutcome::Failed(e)),
    };
    if current != rec.content_hash {
        return kept(KeptReason::LocallyModified);
    }
    if rec.commit.as_deref() == Some(pin.commit.as_str()) {
        return Plan::Done(PinOutcome::Skipped);
    }
    Plan::Install(PinOutcome::Updated { from: rec.commit })
}

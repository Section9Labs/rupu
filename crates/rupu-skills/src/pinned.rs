//! Installing skills pinned to a commit: the stock fleet's skills, which
//! `rupu fleet install` takes from the built-in profiles' `[bundle]` (spec
//! `docs/superpowers/specs/2026-10-13-rupu-fleet-skills-design.md` §5.2).
//! A pin installs through the same path and writes the same source record
//! as `rupu skill install <source> --ref <commit>`, so `rupu skill update`
//! knows where a fleet skill came from.

use crate::install::{find_skills, install_selected, write_git_record, Installed, Selection};
use crate::record::read_record;
use crate::skill::{load_skill, Layer};
use crate::source::{parse_git_source, Fetch, GitSource};
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
    /// The pins that failed: not installed, and not deliberately kept.
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
            let mut record = |inst: &Installed| {
                write_git_record(
                    skills_dir,
                    inst,
                    &pin.source,
                    &url,
                    Some(commit.as_str()),
                    None,
                    tree.commit.as_deref(),
                )
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

/// The https URL a pin's source names (`owner/repo` or an https URL). A pin
/// comes from a built-in profile, so this never looks at the filesystem: a
/// directory named `owner/repo` in the working directory is not the pin.
fn git_url(pin: &Pin) -> Result<String, String> {
    match parse_git_source(&pin.source, Some(&pin.commit)) {
        Ok(g) if g.path.is_none() => Ok(g.url),
        Ok(_) => Err(format!(
            "{}: a pinned source names a repository, not a path in it",
            pin.source
        )),
        Err(e) => Err(format!("{}: {e}", pin.source)),
    }
}

fn plan(pin: &Pin, url: &str, skills_dir: &Path, layer: Layer, force: bool) -> Plan {
    let dir = skills_dir.join(&pin.name);
    let kept = |reason| {
        if force {
            Plan::Install(PinOutcome::Replaced)
        } else {
            Plan::Done(PinOutcome::Kept(reason))
        }
    };
    // Anything at the path (a directory without SKILL.md, a file, a dangling
    // symlink) is not "not installed": only an absent path is created.
    match std::fs::symlink_metadata(&dir) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Plan::Install(PinOutcome::Created)
        }
        Err(e) => return kept_or_failed(force, e.to_string()),
    }
    if !dir.join("SKILL.md").is_file() {
        return kept(KeptReason::OtherSource);
    }
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

/// A path that can't be inspected: refuse it, or replace it under `force`.
fn kept_or_failed(force: bool, reason: String) -> Plan {
    if force {
        Plan::Install(PinOutcome::Replaced)
    } else {
        Plan::Done(PinOutcome::Failed(reason))
    }
}

#[cfg(feature = "git")]
mod checkout {
    use crate::source::{Fetch, FetchedTree, GitSource, SourceError};
    use std::path::PathBuf;

    /// `rupu fleet install --skills-from <dir>`: a local git checkout stands
    /// in for the network. The checkout must contain the pinned commit; its
    /// working tree, index and HEAD are never read. The commit's tree is
    /// exported from the object database into a temporary directory, so
    /// install copies exactly what the pin names (never a modified, staged,
    /// untracked, ignored or `skip-worktree` file).
    pub struct PinnedCheckout {
        pub dir: PathBuf,
    }

    impl Fetch for PinnedCheckout {
        fn fetch(&self, src: &GitSource) -> Result<FetchedTree, SourceError> {
            let shown = self.dir.display();
            let err = |m: String| SourceError::Clone(m);
            let want = src
                .git_ref
                .as_deref()
                .ok_or_else(|| err("--skills-from serves pinned commits only".into()))?;
            let repo = git2::Repository::open(&self.dir)
                .map_err(|e| err(format!("{shown} is not a git checkout: {}", e.message())))?;
            let missing = || {
                err(format!(
                    "{shown} does not contain commit {want}; run `git fetch --tags` in it on a \
                     connected machine (or clone a fresher copy)"
                ))
            };
            let commit = git2::Oid::from_str(want)
                .ok()
                .and_then(|oid| repo.find_commit(oid).ok())
                .ok_or_else(missing)?;
            let tmp = tempfile::tempdir()
                .map_err(|e| err(format!("cannot create a temporary directory: {e}")))?;
            let mut opts = git2::build::CheckoutBuilder::new();
            opts.target_dir(tmp.path())
                .update_index(false)
                .force()
                .recreate_missing(true);
            repo.checkout_tree(commit.as_object(), Some(&mut opts))
                .map_err(|e| err(format!("{shown}: cannot export {want}: {}", e.message())))?;
            Ok(FetchedTree::with_guard(
                tmp.path().to_path_buf(),
                Some(want.to_owned()),
                tmp,
            ))
        }
    }
}

#[cfg(feature = "git")]
pub use checkout::PinnedCheckout;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::{parse_source, Source};

    /// The only test in this binary that changes the working directory.
    #[test]
    fn a_pin_never_resolves_to_a_directory_in_the_working_directory() {
        let cwd = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(cwd.path().join("Section9Labs/rupu-skills")).unwrap();
        let before = std::env::current_dir().unwrap();
        std::env::set_current_dir(cwd.path()).unwrap();
        let pin = Pin {
            name: "x".into(),
            source: "Section9Labs/rupu-skills".into(),
            tag: "v1.0.0".into(),
            commit: "1".repeat(40),
        };
        let probe = parse_source(&pin.source, None, None);
        let url = git_url(&pin);
        std::env::set_current_dir(before).unwrap();
        assert!(
            matches!(probe, Ok(Source::Local(_))),
            "the premise: parse_source takes the directory"
        );
        assert_eq!(
            url.as_deref(),
            Ok("https://github.com/Section9Labs/rupu-skills")
        );
    }

    #[test]
    fn a_pin_source_is_a_repository_url_only() {
        let pin = |s: &str| Pin {
            name: "x".into(),
            source: s.into(),
            tag: "v1".into(),
            commit: "1".repeat(40),
        };
        let ok = |s: &str| git_url(&pin(s));
        assert_eq!(
            ok("https://example.com/o/r/"),
            Ok("https://example.com/o/r".into())
        );
        for bad in [
            "https://tok@example.com/o/r",
            "https://example.com/o/r#skills",
            "./local",
            "git@github.com:o/r",
            "nonsense",
        ] {
            assert!(ok(bad).is_err(), "{bad}");
        }
    }
}

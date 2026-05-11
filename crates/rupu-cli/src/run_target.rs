//! Parse the `target` positional arg of `rupu run` / `rupu workflow run`.
//!
//! Grammar (matches docs/scm.md §"Target syntax"):
//!
//! ```text
//!   github:owner/repo                          # Repo
//!   github:owner/repo#42                       # PR
//!   github:owner/repo/issues/123               # Issue
//!   gitlab:group/project                       # Repo (gitlab.com)
//!   gitlab:group/sub/project!7                 # MR (uses `!` per gitlab convention)
//!   gitlab:group/project/issues/9              # Issue
//! ```

use rupu_scm::{IssueTracker, Platform};
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunTarget {
    Repo {
        platform: Platform,
        owner: String,
        repo: String,
        ref_: Option<String>,
    },
    Pr {
        platform: Platform,
        owner: String,
        repo: String,
        number: u32,
    },
    Issue {
        tracker: IssueTracker,
        project: String,
        number: u64,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum RunTargetParseError {
    #[error("expected `<platform>:<owner>/<repo>[#N | !N | /issues/N]`, got `{0}`")]
    BadShape(String),
    #[error("unknown platform `{0}`")]
    UnknownPlatform(String),
    #[error("invalid number in target: {0}")]
    BadNumber(String),
}

pub fn parse_run_target(s: &str) -> Result<RunTarget, RunTargetParseError> {
    let (platform_str, rest) = s
        .split_once(':')
        .ok_or_else(|| RunTargetParseError::BadShape(s.into()))?;

    // Issue form: <project>/issues/<N> for any supported issue tracker.
    if let Some((project, num_part)) = rest.rsplit_once("/issues/") {
        let number: u64 = num_part
            .parse()
            .map_err(|_| RunTargetParseError::BadNumber(num_part.into()))?;
        let tracker = IssueTracker::from_str(platform_str)
            .map_err(|_| RunTargetParseError::UnknownPlatform(platform_str.into()))?;
        return Ok(RunTarget::Issue {
            tracker,
            project: project.to_string(),
            number,
        });
    }

    let platform = Platform::from_str(platform_str)
        .map_err(|_| RunTargetParseError::UnknownPlatform(platform_str.into()))?;

    // PR form: <path>#<N> (github) or <path>!<N> (gitlab MR)
    let (path, number_opt): (&str, Option<u32>) = if let Some((p, n)) = rest.split_once('#') {
        (
            p,
            Some(
                n.parse()
                    .map_err(|_| RunTargetParseError::BadNumber(n.into()))?,
            ),
        )
    } else if let Some((p, n)) = rest.split_once('!') {
        (
            p,
            Some(
                n.parse()
                    .map_err(|_| RunTargetParseError::BadNumber(n.into()))?,
            ),
        )
    } else {
        (rest, None)
    };

    // Split path into owner+repo. Take the LAST segment as repo, rest as owner.
    let (owner, repo) = path
        .rsplit_once('/')
        .ok_or_else(|| RunTargetParseError::BadShape(s.into()))?;
    if owner.is_empty() || repo.is_empty() {
        return Err(RunTargetParseError::BadShape(s.into()));
    }

    Ok(match number_opt {
        Some(n) => RunTarget::Pr {
            platform,
            owner: owner.to_string(),
            repo: repo.to_string(),
            number: n,
        },
        None => RunTarget::Repo {
            platform,
            owner: owner.to_string(),
            repo: repo.to_string(),
            ref_: None,
        },
    })
}

/// Format a RunTarget into the `## Run target` system-prompt section the
/// runner preloads when invoked with a target arg.
pub fn format_run_target_for_prompt(t: &RunTarget) -> String {
    match t {
        RunTarget::Repo {
            platform,
            owner,
            repo,
            ..
        } => format!(
            "Repo: {platform}:{owner}/{repo}\n\nUse the SCM tools (scm.repos.get, scm.files.read, scm.prs.list) to explore."
        ),
        RunTarget::Pr {
            platform,
            owner,
            repo,
            number,
        } => format!(
            "PR: {platform}:{owner}/{repo}#{number}\n\nUse scm.prs.get + scm.prs.diff to read it. Use scm.prs.comment to post a review."
        ),
        RunTarget::Issue {
            tracker,
            project,
            number,
        } => format!(
            "Issue: {tracker}:{project}/issues/{number}\n\nUse issues.get to read it. If asked to fix, branch + scm.prs.create when done."
        ),
    }
}

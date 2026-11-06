//! Issue refs and repo filters, in the full forms the autoflow stores use
//! (`github:owner/repo/issues/42`, `github:owner/repo`).

use crate::Refusal;
use anyhow::{anyhow, bail};
use rupu_scm::run_target::{parse_run_target, RunTarget};
use rupu_scm::{IssueRef, IssueTracker};

/// `rupu autoflow run`'s target: an issue in run-target form, nothing else.
pub fn parse_full_issue_target(target: &str) -> anyhow::Result<IssueRef> {
    match parse_run_target(target) {
        Ok(RunTarget::Issue {
            tracker,
            project,
            number,
        }) => Ok(IssueRef {
            tracker,
            project,
            number,
        }),
        _ => {
            bail!("autoflow run requires an issue target in `<platform>:<project>/issues/<N>` form")
        }
    }
}

/// An issue ref in its full `<tracker>:<project>/issues/<number>` form.
pub fn parse_issue_ref_text(value: &str) -> anyhow::Result<IssueRef> {
    IssueRef::parse_full(value).map_err(|e| anyhow!(e))
}

/// `issue_ref`'s full form.
pub fn format_issue_ref(issue_ref: &IssueRef) -> String {
    format!(
        "{}:{}/issues/{}",
        issue_ref.tracker, issue_ref.project, issue_ref.number
    )
}

/// The claim key of a full-form issue ref. The CLI resolves a bare `42`
/// against its checkout first; the library never does, so a caller with no
/// checkout (the control plane) can never land on the wrong repo's issue.
pub fn canonical_issue_ref(value: &str) -> anyhow::Result<String> {
    let issue = parse_issue_ref_text(value)?;
    Ok(format!(
        "{}:{}/issues/{}",
        issue.tracker.as_str(),
        issue.project,
        issue.number
    ))
}

/// The repo a GitHub / GitLab issue ref lives in; other trackers name none.
pub fn issue_ref_to_repo_ref(issue_ref: &str) -> anyhow::Result<String> {
    let issue = parse_issue_ref_text(issue_ref)?;
    match issue.tracker {
        IssueTracker::Github | IssueTracker::Gitlab => {
            Ok(format!("{}:{}", issue.tracker, issue.project))
        }
        IssueTracker::Linear | IssueTracker::Jira => Err(anyhow!(
            "tracker issue `{issue_ref}` does not imply a repo binding"
        )),
    }
}

/// `--repo` as `<platform>:<owner>/<repo>`; anything else is a
/// [`Refusal::Invalid`].
pub fn normalize_repo_filter(repo: Option<&str>) -> anyhow::Result<Option<String>> {
    let Some(repo) = repo else {
        return Ok(None);
    };
    let parsed = parse_run_target(repo)
        .map_err(|err| Refusal::Invalid(format!("invalid repo filter `{repo}`: {err}")))?;
    match parsed {
        RunTarget::Repo {
            platform,
            owner,
            repo,
            ..
        } => Ok(Some(format!("{platform}:{owner}/{repo}"))),
        _ => bail!("invalid repo filter `{repo}`: expected `<platform>:<owner>/<repo>`"),
    }
}

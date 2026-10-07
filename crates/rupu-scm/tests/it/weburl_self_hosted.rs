//! Finding permalinks on self-hosted SCMs: a GitHub Enterprise / self-managed
//! GitLab remote resolves to a web URL when an `[scm.<account>]` names its
//! host through `base_url`; an unconfigured host still yields nothing.

use rupu_scm::weburl::{parse_repo_remote, repo_permalink_with, WebHosts};

fn hosts(toml_src: &str) -> WebHosts {
    let scm: rupu_config::ScmSection = toml::from_str(toml_src).unwrap();
    WebHosts::from_scm(&scm)
}

#[test]
fn ghe_base_url_makes_its_remotes_linkable() {
    let h = hosts(
        r#"
[acme-ghe]
kind = "github"
base_url = "https://git.acme.internal/api/v3"
"#,
    );
    assert_eq!(
        repo_permalink_with(
            "git@git.acme.internal:sec/app.git",
            Some("main"),
            "src/a.rs",
            Some([3, 5]),
            &h
        )
        .as_deref(),
        Some("https://git.acme.internal/sec/app/blob/main/src/a.rs#L3-L5")
    );
    // Without the account config the host is unknown.
    assert!(parse_repo_remote("git@git.acme.internal:sec/app.git").is_none());
}

#[test]
fn self_managed_gitlab_by_account_name_and_ssh_port() {
    // `[scm.gitlab]` with no `kind`: the account name is the platform.
    let h = hosts(
        r#"
[gitlab]
base_url = "https://GitLab.Corp.Example:8443"
"#,
    );
    assert_eq!(
        repo_permalink_with(
            "ssh://git@gitlab.corp.example:2222/grp/sub/svc.git",
            Some("dev"),
            "x.py",
            Some([7, 7]),
            &h
        )
        .as_deref(),
        Some("https://gitlab.corp.example/grp/sub/svc/-/blob/dev/x.py#L7")
    );
}

#[test]
fn non_repo_accounts_are_ignored() {
    let h = hosts(
        r#"
[jira]
base_url = "https://jira.acme.internal"
"#,
    );
    assert_eq!(h, WebHosts::default());
}

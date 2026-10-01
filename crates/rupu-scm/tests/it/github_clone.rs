use rupu_scm::connectors::github::{GithubClient, GithubRepoConnector};
use rupu_scm::RepoConnector;

/// Clone URLs always name github.com (GitHub Enterprise clone hosts are
/// TODO.md work), so cloning for a GHES account would hand that instance's
/// token to github.com over HTTPS — or, over SSH, silently clone whatever
/// github.com has at the same path. Refused up front instead, before any
/// network request.
#[tokio::test]
async fn a_github_enterprise_account_s_repo_is_never_cloned_from_github_com() {
    rupu_scm::install_default_crypto_provider();
    for protocol in [rupu_scm::CloneProtocol::Https, rupu_scm::CloneProtocol::Ssh] {
        let client = GithubClient::with_options(
            "ghp-fake-for-a-refusal-test".into(),
            &rupu_scm::ScmClientOptions {
                base_url: Some("https://ghe.example.com/api/v3".into()),
                clone_protocol: protocol,
                ..Default::default()
            },
            std::sync::Arc::new(rupu_netflow::NullSink),
        );
        let dir = tempfile::tempdir().unwrap();
        let err = GithubRepoConnector::new(client)
            .clone_to(
                &rupu_scm::RepoRef {
                    platform: rupu_scm::Platform::Github,
                    owner: "example-org".into(),
                    repo: "example-repo".into(),
                },
                &dir.path().join("checkout"),
            )
            .await
            .expect_err("refused");
        match err {
            rupu_scm::ScmError::BadRequest { message } => {
                assert!(message.contains("ghe.example.com"), "{message}");
                assert!(message.contains("github.com"), "{message}");
            }
            other => panic!("{protocol:?}: expected a refusal, got {other:?}"),
        }
    }
}

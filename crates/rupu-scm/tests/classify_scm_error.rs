use rupu_scm::{classify_scm_error, Platform, ScmError};

fn headers(pairs: &[(&str, &str)]) -> reqwest::header::HeaderMap {
    let mut h = reqwest::header::HeaderMap::new();
    for (k, v) in pairs {
        h.insert(
            reqwest::header::HeaderName::from_bytes(k.as_bytes()).unwrap(),
            reqwest::header::HeaderValue::from_str(v).unwrap(),
        );
    }
    h
}

#[test]
fn github_401_is_unauthorized() {
    let e = classify_scm_error(Platform::Github, 401, "{}", &headers(&[]));
    assert!(matches!(e, ScmError::Unauthorized { .. }));
}

#[test]
fn github_403_with_missing_scope_is_missing_scope() {
    let h = headers(&[
        ("X-OAuth-Scopes", "read:user"),
        ("X-Accepted-OAuth-Scopes", "repo, read:user"),
    ]);
    let e = classify_scm_error(Platform::Github, 403, "{}", &h);
    match e {
        ScmError::MissingScope { scope, .. } => assert!(scope.contains("repo")),
        other => panic!("expected MissingScope, got {other:?}"),
    }
}

#[test]
fn github_403_without_scope_header_is_rate_limited() {
    // A 403 without scope header implies rate-limit (GitHub returns 403 for primary rate limits).
    let e = classify_scm_error(Platform::Github, 403, "{}", &headers(&[]));
    assert!(matches!(e, ScmError::RateLimited { .. }));
}

#[test]
fn github_429_with_retry_after_parses_seconds() {
    let h = headers(&[("Retry-After", "42")]);
    let e = classify_scm_error(Platform::Github, 429, "{}", &h);
    match e {
        ScmError::RateLimited { retry_after } => {
            assert_eq!(retry_after, Some(std::time::Duration::from_secs(42)));
        }
        other => panic!("expected RateLimited, got {other:?}"),
    }
}

#[test]
fn github_404_is_not_found() {
    let e = classify_scm_error(
        Platform::Github,
        404,
        r#"{"message":"Not Found","documentation_url":""}"#,
        &headers(&[]),
    );
    assert!(matches!(e, ScmError::NotFound { .. }));
}

#[test]
fn github_409_is_conflict() {
    let e = classify_scm_error(
        Platform::Github,
        409,
        r#"{"message":"Reference already exists"}"#,
        &headers(&[]),
    );
    match e {
        ScmError::Conflict { message } => assert!(message.contains("already exists")),
        other => panic!("expected Conflict, got {other:?}"),
    }
}

#[test]
fn github_400_is_bad_request() {
    let e = classify_scm_error(
        Platform::Github,
        400,
        r#"{"message":"validation failed"}"#,
        &headers(&[]),
    );
    assert!(matches!(e, ScmError::BadRequest { .. }));
}

#[test]
fn github_502_is_transient() {
    let e = classify_scm_error(Platform::Github, 502, "Bad Gateway", &headers(&[]));
    assert!(matches!(e, ScmError::Transient(_)));
}

#[test]
fn unknown_status_falls_to_transient() {
    let e = classify_scm_error(Platform::Github, 418, "I'm a teapot", &headers(&[]));
    assert!(matches!(e, ScmError::Transient(_)));
}

#[test]
fn is_recoverable_classifies_correctly() {
    assert!(ScmError::RateLimited { retry_after: None }.is_recoverable());
    assert!(ScmError::Transient(anyhow::anyhow!("x")).is_recoverable());
    assert!(ScmError::Conflict {
        message: "x".into()
    }
    .is_recoverable());
    assert!(ScmError::NotFound { what: "x".into() }.is_recoverable());
    assert!(!ScmError::Unauthorized {
        platform: "github".into(),
        hint: "x".into()
    }
    .is_recoverable());
    assert!(!ScmError::MissingScope {
        platform: "github".into(),
        scope: "repo".into(),
        hint: "x".into()
    }
    .is_recoverable());
    assert!(!ScmError::Network(anyhow::anyhow!("x")).is_recoverable());
    assert!(!ScmError::BadRequest {
        message: "x".into()
    }
    .is_recoverable());
}

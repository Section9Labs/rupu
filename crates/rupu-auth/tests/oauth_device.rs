use httpmock::prelude::*;
use rupu_auth::backend::ProviderId;
use rupu_auth::oauth::device;

#[tokio::test]
async fn device_code_flow_completes() {
    let server = MockServer::start();
    let device_mock = server.mock(|when, then| {
        when.method(POST).path("/login/device/code");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(serde_json::json!({
                "device_code": "DEV-CODE",
                "user_code": "ABCD-1234",
                "verification_uri": "https://github.com/login/device",
                "expires_in": 900,
                "interval": 1,
            }));
    });
    let token_mock = server.mock(|when, then| {
        when.method(POST).path("/login/oauth/access_token");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(serde_json::json!({
                "access_token": "ghp-test",
                "token_type": "bearer",
                "scope": "read:user",
            }));
    });

    std::env::set_var(
        "RUPU_DEVICE_DEVICE_URL_OVERRIDE",
        server.url("/login/device/code"),
    );
    std::env::set_var(
        "RUPU_DEVICE_TOKEN_URL_OVERRIDE",
        server.url("/login/oauth/access_token"),
    );
    std::env::set_var("RUPU_DEVICE_FAST_POLL", "1");
    let stored = device::run(ProviderId::Copilot).await.expect("flow ok");
    device_mock.assert();
    token_mock.assert();
    match stored.credentials {
        rupu_providers::auth::AuthCredentials::OAuth { access, .. } => {
            assert_eq!(access, "ghp-test");
        }
        _ => panic!("expected OAuth"),
    }
    std::env::remove_var("RUPU_DEVICE_DEVICE_URL_OVERRIDE");
    std::env::remove_var("RUPU_DEVICE_TOKEN_URL_OVERRIDE");
    std::env::remove_var("RUPU_DEVICE_FAST_POLL");
}

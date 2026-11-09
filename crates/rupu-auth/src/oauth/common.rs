//! What every interactive login shares: the token response and how it
//! becomes a stored credential, the HTTP client, the authorize URL and the
//! authorization-code exchange. The browser-callback flow (`callback`) and
//! the headless flows (`--mode device`) call these, so a login stores the
//! same credential however the user got through the consent screen.

use anyhow::{Context, Result};
use chrono::Utc;
use reqwest_middleware::ClientWithMiddleware;
use rupu_providers::auth::AuthCredentials;
use serde::Deserialize;

use crate::backend::ProviderId;
use crate::oauth::providers::{
    OAuthClient, ProviderOAuth, TokenBodyFormat, EXTRA_CLIENT_ID, EXTRA_TOKEN_URL,
};
use crate::stored::StoredCredential;

#[derive(Debug, Deserialize)]
pub(crate) struct TokenResponse {
    pub(crate) access_token: String,
    #[serde(default)]
    pub(crate) refresh_token: Option<String>,
    #[serde(default)]
    pub(crate) expires_in: Option<i64>,
    /// OpenAI's ChatGPT grant returns an ID token next to the access token;
    /// stored (`extra.id_token`) as codex-rs stores it, so the Codex client
    /// can take its account id from it when the access token has no claim.
    /// Stored for OpenAI only ([`stored_credential_for`]): Gemini's
    /// (`openid` + `email` scopes) carries the user's email and is dropped.
    #[serde(default)]
    pub(crate) id_token: Option<String>,
    /// Anthropic-shaped account block (uuid, email, …). Optional — other
    /// OAuth providers don't return this. We capture the uuid only; other
    /// fields are intentionally ignored to avoid storing extra PII.
    #[serde(default)]
    account: Option<AccountInfo>,
    /// Anthropic-shaped organization block (uuid, name). Same rationale.
    #[serde(default)]
    organization: Option<OrganizationInfo>,
}

#[derive(Debug, Deserialize)]
struct AccountInfo {
    #[serde(default)]
    uuid: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OrganizationInfo {
    #[serde(default)]
    uuid: Option<String>,
}

/// What of a token response is stored for `provider`: the tokens and their
/// expiry, plus the Anthropic account/organization UUIDs (so the Anthropic
/// adapter can send `metadata.user_id.account_uuid`, binding traffic to
/// the user's Pro/Max quota pool) and — for OpenAI only — the ID token the
/// Codex client takes its account id from. Any other provider's ID token
/// (Gemini's carries the user's email) is not stored. A Gemini credential
/// records the OAuth client it was issued to (`variant`: the Gemini CLI's);
/// its Code Assist project is set up once it is stored
/// ([`crate::oauth::gemini::set_up_code_assist`]).
pub(crate) fn stored_credential_for(
    provider: ProviderId,
    token: TokenResponse,
) -> StoredCredential {
    let expires_at = token
        .expires_in
        .map(|s| Utc::now() + chrono::Duration::seconds(s));

    let expires_ms = expires_at.map(|d| d.timestamp_millis() as u64).unwrap_or(0);

    let mut extra: std::collections::HashMap<String, serde_json::Value> = Default::default();
    if let Some(uuid) = token.account.as_ref().and_then(|a| a.uuid.clone()) {
        extra.insert("account_uuid".into(), serde_json::Value::String(uuid));
    }
    if let Some(uuid) = token.organization.as_ref().and_then(|o| o.uuid.clone()) {
        extra.insert("organization_uuid".into(), serde_json::Value::String(uuid));
    }
    if provider == ProviderId::Openai {
        if let Some(id_token) = token.id_token {
            extra.insert("id_token".into(), serde_json::Value::String(id_token));
        }
    }
    if provider == ProviderId::Gemini {
        let variant = rupu_providers::google_gemini::GeminiVariant::GeminiCli;
        if let Some(hint) = variant.credential_hint() {
            extra.insert(
                rupu_providers::google_gemini::code_assist::EXTRA_VARIANT.into(),
                hint.into(),
            );
        }
    }

    StoredCredential {
        credentials: AuthCredentials::OAuth {
            access: token.access_token,
            refresh: token.refresh_token.clone().unwrap_or_default(),
            expires: expires_ms,
            extra,
        },
        refresh_token: token.refresh_token,
        expires_at,
    }
}

/// Records a chosen application's id and token endpoint
/// ([`EXTRA_CLIENT_ID`] / [`EXTRA_TOKEN_URL`]) on an OAuth credential, so
/// its refreshes go to the application and endpoint that issued it.
pub(crate) fn record_client(stored: &mut StoredCredential, app: &OAuthClient) {
    if let AuthCredentials::OAuth { extra, .. } = &mut stored.credentials {
        extra.insert(EXTRA_CLIENT_ID.into(), app.client_id.clone().into());
        extra.insert(EXTRA_TOKEN_URL.into(), app.token_url.clone().into());
    }
}

/// The one HTTP client every auth flow uses.
///
/// `Arc::new(NullSink)`, deliberately: auth/OAuth traffic is out of
/// netflow's scope by matt's explicit ruling — "I do not care about
/// update or login" — not a stopgap pending further wiring. See
/// `resolver.rs`'s `refresh_inner` doc comment for the same ruling
/// stated at its call site; there is no further wiring planned here.
pub(crate) fn http_client() -> Result<ClientWithMiddleware> {
    Ok(rupu_netflow::http::client_with(
        rupu_netflow::FlowCtx::system(rupu_netflow::Origin::System),
        reqwest::Client::builder(),
        std::sync::Arc::new(rupu_netflow::NullSink),
    )?)
}

/// The browser flow's authorize URL for `app`, with `extra` appended after
/// `oauth.extra_authorize_params`.
pub(crate) fn build_authorize_url(
    oauth: &ProviderOAuth,
    app: &OAuthClient,
    challenge: &str,
    state: &str,
    redirect_uri: &str,
    extra: &[(&str, &str)],
) -> Result<String> {
    let mut url = url::Url::parse(&app.authorize_url)?;
    {
        let mut q = url.query_pairs_mut();
        q.append_pair("response_type", "code")
            .append_pair("client_id", &app.client_id)
            .append_pair("redirect_uri", redirect_uri)
            .append_pair("scope", &oauth.scopes.join(" "))
            .append_pair("state", state)
            .append_pair("code_challenge", challenge)
            .append_pair("code_challenge_method", "S256");
        for (k, v) in oauth.extra_authorize_params.iter().chain(extra) {
            q.append_pair(k, v);
        }
    }
    Ok(url.to_string())
}

/// One authorization-code grant: what [`exchange_code`] sends to
/// `app.token_url`.
pub(crate) struct CodeExchange<'a> {
    pub oauth: &'a ProviderOAuth,
    pub app: &'a OAuthClient,
    pub client_secret: Option<&'a str>,
    pub code: &'a str,
    pub redirect_uri: &'a str,
    pub code_verifier: &'a str,
    pub state: &'a str,
}

/// POSTs the authorization-code grant in `oauth.token_body_format`, adding
/// `client_secret` when given and `state` when
/// `oauth.include_state_in_token_body`.
pub(crate) async fn exchange_code(
    client: &ClientWithMiddleware,
    x: CodeExchange<'_>,
) -> Result<TokenResponse> {
    let mut params: Vec<(&str, String)> = vec![
        ("grant_type", "authorization_code".into()),
        ("code", x.code.into()),
        ("client_id", x.app.client_id.clone()),
        ("redirect_uri", x.redirect_uri.into()),
        ("code_verifier", x.code_verifier.into()),
    ];
    if let Some(secret) = x.client_secret {
        params.push(("client_secret", secret.into()));
    }
    if x.oauth.include_state_in_token_body {
        params.push(("state", x.state.into()));
    }

    let token_url = &x.app.token_url;
    let request = match x.oauth.token_body_format {
        TokenBodyFormat::Form => client.post(token_url).form(&params),
        TokenBodyFormat::Json => {
            let json: std::collections::BTreeMap<&str, String> = params.into_iter().collect();
            client.post(token_url).json(&json)
        }
    };

    request
        .send()
        .await
        .context("token exchange request")?
        .error_for_status()
        .context("token exchange status")?
        .json()
        .await
        .context("token exchange json")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oauth::providers::provider_oauth;

    fn token_with_id_token() -> TokenResponse {
        serde_json::from_value(serde_json::json!({
            "access_token": "access",
            "refresh_token": "refresh",
            "expires_in": 3600,
            "id_token": "eyJ.id-token"
        }))
        .unwrap()
    }

    fn extra_of(sc: &StoredCredential) -> &std::collections::HashMap<String, serde_json::Value> {
        match &sc.credentials {
            AuthCredentials::OAuth { extra, .. } => extra,
            other => panic!("expected OAuth, got {other:?}"),
        }
    }

    fn corp_app() -> OAuthClient {
        OAuthClient {
            client_id: "corp-app".into(),
            authorize_url: "https://gitlab.example.com/oauth/authorize".into(),
            token_url: "https://gitlab.example.com/oauth/token".into(),
            device_url: None,
        }
    }

    /// OpenAI's ID token is stored (the Codex client takes its account id
    /// from it); no other provider's is.
    #[test]
    fn only_openai_s_id_token_is_stored() {
        let openai = stored_credential_for(ProviderId::Openai, token_with_id_token());
        assert_eq!(
            extra_of(&openai).get("id_token").and_then(|v| v.as_str()),
            Some("eyJ.id-token")
        );
        for provider in [ProviderId::Gemini, ProviderId::Anthropic] {
            let sc = stored_credential_for(provider, token_with_id_token());
            assert!(
                !extra_of(&sc).contains_key("id_token"),
                "{provider}: {:?}",
                extra_of(&sc)
            );
            assert_eq!(sc.refresh_token.as_deref(), Some("refresh"));
        }
    }

    #[test]
    fn record_client_names_the_chosen_application_and_its_token_endpoint() {
        let mut sc = stored_credential_for(ProviderId::Gitlab, token_with_id_token());
        record_client(&mut sc, &corp_app());
        assert_eq!(extra_of(&sc)[EXTRA_CLIENT_ID], "corp-app");
        assert_eq!(
            extra_of(&sc)[EXTRA_TOKEN_URL],
            "https://gitlab.example.com/oauth/token"
        );
    }

    /// A login with a chosen application (a GitLab account's configured
    /// one, or a self-managed instance's) sends the user to that
    /// application's consent screen, on that instance.
    #[test]
    fn the_authorize_url_names_the_chosen_application_on_its_instance() {
        let oauth = provider_oauth(ProviderId::Gitlab).unwrap();
        let url = build_authorize_url(
            &oauth,
            &corp_app(),
            "challenge",
            "state",
            "http://localhost:7171/auth/redirect",
            &[],
        )
        .unwrap();
        let url = url::Url::parse(&url).unwrap();
        assert_eq!(url.host_str(), Some("gitlab.example.com"));
        assert_eq!(url.path(), "/oauth/authorize");
        let q: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(q["client_id"], "corp-app");
        assert_eq!(q["redirect_uri"], "http://localhost:7171/auth/redirect");
        assert_eq!(q["scope"], "openid profile read_user write_repository api");
        assert_eq!(q["code_challenge_method"], "S256");
    }

    /// A flow's own parameters follow the provider's fixed ones.
    #[test]
    fn extra_authorize_params_follow_the_provider_s() {
        let oauth = provider_oauth(ProviderId::Anthropic).unwrap();
        let app = OAuthClient {
            client_id: oauth.client_id.into(),
            authorize_url: oauth.authorize_url.into(),
            token_url: oauth.token_url.into(),
            device_url: None,
        };
        let url = build_authorize_url(
            &oauth,
            &app,
            "challenge",
            "state",
            "https://example.com/callback",
            &[("flow", "paste")],
        )
        .unwrap();
        let url = url::Url::parse(&url).unwrap();
        let keys: Vec<String> = url.query_pairs().map(|(k, _)| k.into_owned()).collect();
        let code = keys.iter().position(|k| k == "code").unwrap();
        let flow = keys.iter().position(|k| k == "flow").unwrap();
        assert!(code < flow, "{keys:?}");
        assert_eq!(keys.last().map(String::as_str), Some("flow"));
    }
}

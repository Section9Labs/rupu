//! A Gemini SSO login's Code Assist setup: the Google Cloud project the
//! account's Code Assist requests are billed and metered against, found (or
//! provisioned) with the new token and stored in the credential's `extra` —
//! what gemini-cli does at sign-in (`setupUser`, ported in
//! `rupu_providers::google_gemini::code_assist`).
//!
//! The login itself never fails on it: the token is good either way, and a
//! Gemini client sets up a credential stored without a project on its first
//! request (`GoogleGeminiClient::ensure_project`), failing that request with
//! the actionable error if it still can't.

use rupu_providers::auth::AuthCredentials;
use rupu_providers::google_gemini::code_assist;
use rupu_providers::google_gemini::GeminiVariant;

use crate::stored::StoredCredential;

/// Bound on each setup request. The onboarding poll has its own bound
/// ([`code_assist::OnboardPoll`]).
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Set up Code Assist for the account `stored` was just issued to, and
/// record the result in its `extra`: the variant the token belongs to
/// (`rupu auth login` always signs in as the Gemini CLI client) and — unless
/// `GOOGLE_CLOUD_PROJECT` chose it — the project. Reports on stderr; never
/// fails the login.
pub(crate) async fn set_up_code_assist(stored: &mut StoredCredential) {
    let AuthCredentials::OAuth { access, extra, .. } = &mut stored.credentials else {
        return;
    };
    let variant = GeminiVariant::GeminiCli;
    if let Some(hint) = variant.credential_hint() {
        extra.insert(code_assist::EXTRA_VARIANT.into(), hint.into());
    }
    let chosen = match code_assist::project_override(|k| std::env::var(k).ok()) {
        Ok(chosen) => chosen,
        Err(e) => return report_failure(&e.to_string()),
    };
    // `NullSink`, as for the token exchange (`callback.rs`): login traffic
    // is outside netflow's scope by matt's ruling.
    let client = match rupu_netflow::http::client_with(
        rupu_netflow::FlowCtx::system(rupu_netflow::Origin::System),
        reqwest::Client::builder().timeout(REQUEST_TIMEOUT),
        std::sync::Arc::new(rupu_netflow::NullSink),
    ) {
        Ok(client) => client,
        Err(e) => return report_failure(&e.to_string()),
    };
    let set_up = code_assist::setup_user(
        &client,
        &code_assist::endpoint(variant),
        variant,
        access,
        chosen.as_ref().map(|c| c.project.as_str()),
        &code_assist::OnboardPoll::default(),
    )
    .await;
    match (set_up, chosen) {
        (Ok(project), None) => {
            eprintln!("rupu: Gemini Code Assist project: {project}");
            extra.insert(code_assist::EXTRA_PROJECT_ID.into(), project.into());
        }
        (Ok(project), Some(chosen)) => eprintln!(
            "rupu: Gemini Code Assist project: {project} (from {var}; not stored — runs use \
             {var} while it is set)",
            var = chosen.var
        ),
        (Err(e), _) => report_failure(&e.to_string()),
    }
}

fn report_failure(error: &str) {
    eprintln!(
        "rupu: signed in, but Gemini Code Assist setup failed: {error}\nrupu: the first Gemini \
         request retries the setup."
    );
}

//! A Gemini SSO login's Code Assist setup: the Google Cloud project the
//! account's Code Assist requests are billed and metered against, found (or
//! provisioned) with the new token and recorded in the stored credential's
//! `extra` — what gemini-cli does at sign-in (`setupUser`, ported in
//! `rupu_providers::google_gemini::code_assist`).
//!
//! It runs once the login is stored, and never undoes it: a setup that
//! fails, or is interrupted (Ctrl-C during an onboarding), leaves the login
//! in place, and a Gemini client sets up a credential stored without a
//! project on its first request (`GoogleGeminiClient::ensure_project`),
//! failing that request with the actionable error if it still can't.

use rupu_providers::auth::AuthCredentials;
use rupu_providers::google_gemini::code_assist;
use rupu_providers::google_gemini::GeminiVariant;

use crate::resolver::CredentialResolver;
use crate::stored::StoredCredential;

/// Bound on each setup request. The onboarding poll has its own bound
/// ([`code_assist::OnboardPoll`]).
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Set up Code Assist for the account `stored` — just logged in and stored
/// in `resolver` under `account` — was issued to, and record the project in
/// the stored credential through the store's lock (unless
/// `GOOGLE_CLOUD_PROJECT` chose it: an override is never stored). Reports on
/// stderr; never fails, and never touches the stored login otherwise.
pub async fn set_up_code_assist(
    resolver: &dyn CredentialResolver,
    account: &str,
    stored: &StoredCredential,
) {
    let AuthCredentials::OAuth { access, .. } = &stored.credentials else {
        return;
    };
    // `rupu auth login` signs in as the Gemini CLI client.
    let variant = GeminiVariant::GeminiCli;
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
    eprintln!(
        "rupu: setting up Gemini Code Assist (the Google Cloud project for this account)… the \
         sign-in is already saved; Ctrl-C leaves the setup to the first Gemini request"
    );
    let set_up = code_assist::setup_user(
        &client,
        &code_assist::endpoint(variant),
        variant,
        access,
        chosen.as_ref().map(|c| c.project.as_str()),
        &code_assist::OnboardPoll::default(),
    )
    .await;
    let project = match (set_up, chosen) {
        (Err(e), _) => return report_failure(&e.to_string()),
        (Ok(project), Some(chosen)) => {
            return eprintln!(
                "rupu: Gemini Code Assist project: {project} (set up with {var}={requested}; not \
                 stored — runs set it up from {var} while it is set)",
                var = chosen.var,
                requested = chosen.project,
            )
        }
        (Ok(project), None) => project,
    };
    let Some(store) = resolver.oauth_refresher(account, "gemini") else {
        return report_failure(&format!(
            "Google assigned project {project}, but the credential store cannot record it"
        ));
    };
    let mut fields = std::collections::HashMap::new();
    fields.insert(code_assist::EXTRA_PROJECT_ID.into(), project.clone().into());
    if let Some(hint) = variant.credential_hint() {
        fields.insert(code_assist::EXTRA_VARIANT.into(), hint.into());
    }
    match store.record_extra(stored.credentials.clone(), fields).await {
        Ok(true) => eprintln!("rupu: Gemini Code Assist project: {project}"),
        Ok(false) => report_failure(&format!(
            "Google assigned project {project}, but the stored {account} credential changed \
             during the setup (another login or logout), so it was not recorded"
        )),
        Err(e) => report_failure(&format!(
            "Google assigned project {project}, but it could not be recorded: {e}"
        )),
    }
}

fn report_failure(error: &str) {
    eprintln!(
        "rupu: signed in, but Gemini Code Assist setup failed: {error}\nrupu: the first Gemini \
         request retries the setup."
    );
}

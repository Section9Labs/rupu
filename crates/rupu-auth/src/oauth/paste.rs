//! Paste-the-code login: Claude Code's and Gemini CLI's headless login. The authorize URL
//! names a vendor-hosted page as its `redirect_uri`
//! ([`HeadlessFlow::PasteCode`]), so no listener is bound: the operator
//! opens the URL in a browser on any machine, signs in, copies what the
//! page shows (Anthropic's `code#state`) and pastes it back, and rupu
//! exchanges it as the browser flow (`callback`) exchanges its redirect's
//! code. One attempt: a code is single-use and short-lived, so a failure
//! ends the login and the operator reruns it.

use std::io::{BufRead, Write};

use anyhow::{anyhow, bail, Result};

use crate::backend::ProviderId;
use crate::oauth::common::{self, CodeExchange};
use crate::oauth::pkce::PkcePair;
use crate::oauth::providers::{provider_oauth, HeadlessFlow, OAuthClient};
use crate::stored::StoredCredential;

/// Where the pasted code comes from. Production: stdin (TTY or not).
pub trait CodeInput {
    /// Shows `authorize_url` to the operator and returns what they pasted.
    /// `state` is what this login sent (tests build `code#state` from it).
    fn code(&mut self, authorize_url: &str, state: &str) -> Result<String>;
}

/// Prints the authorize URL and a prompt to stderr and reads one line from
/// stdin, whether or not it is a terminal (an operator can pipe the code).
pub struct StdinInput;

impl CodeInput for StdinInput {
    fn code(&mut self, authorize_url: &str, _state: &str) -> Result<String> {
        eprint!(
            "Open this URL in any browser and sign in:\n\n  {authorize_url}\n\nPaste the code the page shows: "
        );
        std::io::stderr().flush()?;
        let mut line = String::new();
        std::io::stdin().lock().read_line(&mut line)?;
        Ok(line)
    }
}

/// A pasted code, and the state it came with when the paste had one.
#[derive(Debug, PartialEq, Eq)]
pub struct Pasted {
    pub code: String,
    pub state: Option<String>,
}

/// Reads what the operator pasted: surrounding whitespace, quotes and
/// backticks are dropped; a URL (the page's address bar) is read for its
/// `code` / `state` query; so is a `code=…&state=…` query string; else
/// `code#state` splits at the first `#`, and anything else is a bare code.
pub fn parse_pasted(input: &str) -> Result<Pasted> {
    let s = input.trim_matches(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '`'));
    if s.is_empty() {
        bail!("nothing was pasted");
    }
    if s.starts_with("https://") || s.starts_with("http://") {
        let url = url::Url::parse(s).map_err(|e| anyhow!("the pasted URL does not parse: {e}"))?;
        return from_query(url.query().unwrap_or_default())
            .ok_or_else(|| anyhow!("the pasted URL has no `code`"));
    }
    if let Some(pasted) = from_query(s.trim_start_matches('?')) {
        return Ok(pasted);
    }
    let (code, state) = match s.split_once('#') {
        Some((code, state)) => (code, Some(state)),
        None => (s, None),
    };
    if code.is_empty() {
        bail!("nothing was pasted before the `#`");
    }
    Ok(Pasted {
        code: code.to_string(),
        state: state.filter(|st| !st.is_empty()).map(str::to_string),
    })
}

/// A query string's non-empty `code` and its `state`, if it has a `code`.
fn from_query(query: &str) -> Option<Pasted> {
    let mut code = None;
    let mut state = None;
    for (k, v) in url::form_urlencoded::parse(query.as_bytes()) {
        match k.as_ref() {
            "code" if code.is_none() => code = Some(v.into_owned()),
            "state" if state.is_none() => state = Some(v.into_owned()),
            _ => {}
        }
    }
    Some(Pasted {
        code: code.filter(|c| !c.is_empty())?,
        state: state.filter(|st| !st.is_empty()),
    })
}

/// Runs the login for `provider`, reading the code from stdin.
pub async fn run(provider: ProviderId, client: Option<OAuthClient>) -> Result<StoredCredential> {
    run_with_input(provider, client, &mut StdinInput).await
}

/// Runs the login with the code from `input`. `client` is an application
/// chosen at login, as for `callback::run_with_client`: sent no secret,
/// never redirected by `RUPU_OAUTH_TOKEN_URL_OVERRIDE`, and recorded on the
/// credential. A pasted state other than the one this login sent ends it
/// before any exchange; a paste with no state proceeds with rupu's.
pub async fn run_with_input(
    provider: ProviderId,
    client: Option<OAuthClient>,
    input: &mut dyn CodeInput,
) -> Result<StoredCredential> {
    let oauth =
        provider_oauth(provider).ok_or_else(|| anyhow!("no oauth config for {provider}"))?;
    let Some(HeadlessFlow::PasteCode {
        redirect_uri,
        extra_authorize_params,
    }) = oauth.headless
    else {
        bail!("provider {provider} has no paste-code flow");
    };
    let chosen = client.is_some();
    let app = client.unwrap_or_else(|| OAuthClient {
        client_id: oauth.client_id.to_string(),
        authorize_url: oauth.authorize_url.to_string(),
        token_url: std::env::var("RUPU_OAUTH_TOKEN_URL_OVERRIDE")
            .unwrap_or_else(|_| oauth.token_url.to_string()),
        device_url: None,
    });
    let client_secret = if chosen { None } else { oauth.client_secret };

    let pkce = PkcePair::generate();
    let state = if oauth.state_is_verifier {
        pkce.verifier.clone()
    } else {
        common::random_state()
    };
    let authorize = common::build_authorize_url(
        &oauth,
        &app,
        &pkce.challenge,
        &state,
        redirect_uri,
        extra_authorize_params,
    )?;

    let pasted = parse_pasted(&input.code(&authorize, &state)?)?;
    if pasted.state.as_deref().is_some_and(|got| got != state) {
        bail!("pasted state does not match this login; start again");
    }

    let http = common::http_client()?;
    let token = common::exchange_code(
        &http,
        CodeExchange {
            oauth: &oauth,
            app: &app,
            client_secret,
            code: &pasted.code,
            redirect_uri,
            code_verifier: &pkce.verifier,
            state: &state,
        },
    )
    .await?;

    // Google issues a refresh token only on a consent it shows; without one
    // the login would die in an hour. Anthropic's flow tolerates its absence.
    if provider == ProviderId::Gemini
        && token
            .refresh_token
            .as_deref()
            .is_none_or(|r| r.trim().is_empty())
    {
        bail!(
            "Google returned no refresh token. Revoke \"Gemini CLI\" at \
             https://myaccount.google.com/permissions and log in again, or use --mode api-key"
        );
    }

    let mut stored = common::stored_credential_for(provider, token);
    if chosen {
        common::record_client(&mut stored, &app);
    }
    Ok(stored)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_hash_state_splits_at_the_first_hash() {
        assert_eq!(
            parse_pasted("abc#st").unwrap(),
            Pasted {
                code: "abc".into(),
                state: Some("st".into())
            }
        );
        assert_eq!(parse_pasted("a#b#c").unwrap().state.as_deref(), Some("b#c"));
    }

    #[test]
    fn a_bare_code_has_no_state() {
        assert_eq!(
            parse_pasted("abc").unwrap(),
            Pasted {
                code: "abc".into(),
                state: None
            }
        );
    }

    #[test]
    fn a_redirect_url_is_read_for_code_and_state() {
        let p = parse_pasted("https://platform.claude.com/oauth/code/callback?code=abc&state=st")
            .unwrap();
        assert_eq!(
            p,
            Pasted {
                code: "abc".into(),
                state: Some("st".into())
            }
        );
    }

    #[test]
    fn a_query_string_is_read_for_code_and_state() {
        assert_eq!(
            parse_pasted("code=abc&state=st").unwrap(),
            Pasted {
                code: "abc".into(),
                state: Some("st".into())
            }
        );
    }

    #[test]
    fn whitespace_quotes_and_backticks_are_stripped() {
        for s in ["  abc#st \n", "\"abc#st\"", "`abc#st`", "'abc#st'"] {
            assert_eq!(parse_pasted(s).unwrap().code, "abc", "{s:?}");
        }
    }

    #[test]
    fn an_empty_paste_is_an_error() {
        assert!(parse_pasted("  \n")
            .unwrap_err()
            .to_string()
            .contains("nothing was pasted"));
    }
}

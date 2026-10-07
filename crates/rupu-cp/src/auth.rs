//! `rupu cp serve --token` — how a request proves it holds the token.
//!
//! Two credentials are accepted on `/api/*`:
//!
//! - `Authorization: Bearer <token>` — scripts, `curl`, other control planes.
//! - The token cookie — the browser. The SPA's `fetch` calls and its
//!   `EventSource` streams cannot carry an `Authorization` header, but they
//!   send same-origin cookies. The cookie is set by a one-time bootstrap: a
//!   page load with `?token=<token>` (the URL `cp serve` prints and opens)
//!   answers `303` to the same URL without the parameter and a `Set-Cookie`
//!   (`HttpOnly; SameSite=Strict; Path=/`), so the token leaves the address
//!   bar and history immediately.
//!
//! The cookie is ambient, so a cookie-authenticated request that changes
//! state (anything but `GET`/`HEAD`/`OPTIONS`) must also carry an `Origin`
//! naming this server: a page on another origin — another port on
//! `localhost` included, which `SameSite` treats as the same site — cannot
//! drive mutations with it. Bearer requests are not ambient and skip that.
//!
//! The cookie name carries the port from the `Host` header, so two control
//! planes on one machine (cookies ignore ports) never overwrite each other's.
//! Its value is not the token but `HMAC-SHA256(token, "rupu-cp-browser")`
//! (base64url): browsers send cookies to every port on a host, so another
//! local server could read the cookie — it then holds a browser credential
//! for this control plane, never the bearer token itself (the cookie value
//! is refused as a bearer). Every comparison is constant-time. Without
//! `--token` none of this is installed and the API is open.
//!
//! Behind a reverse proxy, keep the `Host` header (`proxy_set_header Host
//! $host`): the cookie name and the same-origin check both read it.

use crate::error::ApiError;
use axum::{
    extract::{Request, State},
    http::{
        header::{AUTHORIZATION, CACHE_CONTROL, COOKIE, HOST, LOCATION, ORIGIN, SET_COOKIE},
        HeaderMap, HeaderValue, Method, StatusCode,
    },
    middleware::Next,
    response::{IntoResponse, Response},
};
use std::sync::Arc;
use subtle::ConstantTimeEq;

/// The query parameter that bootstraps the cookie on a page load.
pub const TOKEN_QUERY_PARAM: &str = "token";

/// Cookie name prefix; the serving port is appended (see [`cookie_name`]).
pub const TOKEN_COOKIE_PREFIX: &str = "rupu_cp_token";

/// 30 days. A restarted `cp serve` with a different token simply 401s the
/// stale cookie until the operator opens the printed link again.
const COOKIE_MAX_AGE_SECS: u64 = 30 * 24 * 60 * 60;

/// The configured token plus the browser cookie value derived from it
/// (computed once).
#[derive(Debug)]
pub struct Token {
    raw: String,
    cookie_value: String,
}

/// The HMAC message the cookie value is derived with.
const COOKIE_CONTEXT: &[u8] = b"rupu-cp-browser";

impl Token {
    pub fn new(raw: String) -> Arc<Self> {
        use base64::Engine as _;
        use hmac::Mac as _;
        let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(raw.as_bytes())
            .expect("HMAC takes a key of any length");
        mac.update(COOKIE_CONTEXT);
        let cookie_value =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
        Arc::new(Self { raw, cookie_value })
    }

    fn matches(&self, presented: &str) -> bool {
        bool::from(presented.as_bytes().ct_eq(self.raw.as_bytes()))
    }

    fn matches_cookie(&self, presented: &str) -> bool {
        bool::from(presented.as_bytes().ct_eq(self.cookie_value.as_bytes()))
    }
}

/// The port in a `Host` header value (`127.0.0.1:7878`, `[::1]:7878`), if any.
fn host_port(host: &str) -> Option<&str> {
    let (_, port) = host.rsplit_once(':')?;
    (!port.is_empty() && port.bytes().all(|b| b.is_ascii_digit())).then_some(port)
}

/// `rupu_cp_token_<port>`, or the bare prefix when the `Host` has no port.
pub fn cookie_name(headers: &HeaderMap) -> String {
    match headers
        .get(HOST)
        .and_then(|v| v.to_str().ok())
        .and_then(host_port)
    {
        Some(port) => format!("{TOKEN_COOKIE_PREFIX}_{port}"),
        None => TOKEN_COOKIE_PREFIX.to_string(),
    }
}

/// Every value of cookie `name` across all `Cookie` headers.
fn cookie_values<'a>(headers: &'a HeaderMap, name: &'a str) -> impl Iterator<Item = &'a str> {
    headers
        .get_all(COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(move |pair| {
            let (k, v) = pair.trim().split_once('=')?;
            (k == name).then_some(v)
        })
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
}

/// True when the request's `Origin` names the host it was sent to.
fn same_origin(headers: &HeaderMap) -> bool {
    let Some(host) = headers.get(HOST).and_then(|v| v.to_str().ok()) else {
        return false;
    };
    let Some(origin) = headers.get(ORIGIN).and_then(|v| v.to_str().ok()) else {
        return false;
    };
    let authority = origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("https://"));
    authority.is_some_and(|a| a.eq_ignore_ascii_case(host))
}

fn unauthorized() -> Response {
    ApiError(
        StatusCode::UNAUTHORIZED,
        "unauthorized: this control plane requires its token — send \
         `Authorization: Bearer <token>`, or open the link `rupu cp serve` \
         printed (it carries `?token=`) to sign this browser in"
            .into(),
    )
    .into_response()
}

/// The `/api/*` guard: a matching bearer header, or a matching token cookie
/// (plus a same-origin `Origin` on a state-changing method). Anything else is
/// a `401` in the standard `{"error": ...}` envelope.
pub async fn require_token(State(token): State<Arc<Token>>, req: Request, next: Next) -> Response {
    let headers = req.headers();
    if bearer(headers).is_some_and(|p| token.matches(p)) {
        return next.run(req).await;
    }
    let name = cookie_name(headers);
    if cookie_values(headers, &name).any(|v| token.matches_cookie(v)) {
        let safe = matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS);
        if !safe && !same_origin(headers) {
            return ApiError(
                StatusCode::FORBIDDEN,
                "cross-origin request refused: a cookie-authenticated write must \
                 come from this control plane's own pages"
                    .into(),
            )
            .into_response();
        }
        return next.run(req).await;
    }
    unauthorized()
}

/// The cookie bootstrap, layered over the whole router. A non-API request
/// carrying `?token=` is answered here: the right token gets a `303` back to
/// the same path and query minus `token`, with the cookie set; a wrong one is
/// a `401`. Every other request passes through untouched.
pub async fn bootstrap_cookie(
    State(token): State<Arc<Token>>,
    req: Request,
    next: Next,
) -> Response {
    let path = req.uri().path().to_string();
    let query = req.uri().query().map(str::to_string);
    let api = path == "/api" || path.starts_with("/api/");
    let (presented, rest) = match (api, query) {
        (false, Some(q)) => split_token_param(&q),
        _ => (None, String::new()),
    };
    let Some(presented) = presented else {
        return next.run(req).await;
    };
    if !token.matches(&presented) {
        return (
            StatusCode::UNAUTHORIZED,
            [(CACHE_CONTROL, "no-store")],
            "rupu control plane: that token is not this server's token",
        )
            .into_response();
    }
    // Never a protocol-relative (`//host`, `/\\host`) target: the redirect
    // stays on this server.
    let path = if path.starts_with("//") || path.starts_with("/\\") {
        "/"
    } else {
        path.as_str()
    };
    let location = if rest.is_empty() {
        path.to_string()
    } else {
        format!("{path}?{rest}")
    };
    let cookie = format!(
        "{}={}; Path=/; HttpOnly; SameSite=Strict; Max-Age={COOKIE_MAX_AGE_SECS}",
        cookie_name(req.headers()),
        token.cookie_value,
    );
    let mut resp = StatusCode::SEE_OTHER.into_response();
    let h = resp.headers_mut();
    if let Ok(v) = HeaderValue::from_str(&location) {
        h.insert(LOCATION, v);
    } else {
        h.insert(LOCATION, HeaderValue::from_static("/"));
    }
    if let Ok(v) = HeaderValue::from_str(&cookie) {
        h.insert(SET_COOKIE, v);
    }
    h.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    resp
}

/// Split `query` into the `token` parameter's value (the last one wins) and
/// the rest, re-encoded in order.
fn split_token_param(query: &str) -> (Option<String>, String) {
    let mut presented = None;
    let mut rest = url::form_urlencoded::Serializer::new(String::new());
    for (k, v) in url::form_urlencoded::parse(query.as_bytes()) {
        if k == TOKEN_QUERY_PARAM {
            presented = Some(v.into_owned());
        } else {
            rest.append_pair(&k, &v);
        }
    }
    (presented, rest.finish())
}

/// The URL `cp serve` prints and opens: `base` plus the bootstrap parameter.
pub fn bootstrap_url(base: &str, token: &str) -> String {
    let enc: String = url::form_urlencoded::byte_serialize(token.as_bytes()).collect();
    format!("{base}/?{TOKEN_QUERY_PARAM}={enc}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_port_handles_v4_v6_and_none() {
        assert_eq!(host_port("127.0.0.1:7878"), Some("7878"));
        assert_eq!(host_port("[::1]:9000"), Some("9000"));
        assert_eq!(host_port("[::1]"), None);
        assert_eq!(host_port("example.com"), None);
    }

    #[test]
    fn the_cookie_is_derived_from_the_token_never_the_token() {
        let t = Token::new("a b;c=d".into());
        // Cookie-safe (base64url), deterministic, and not the token.
        assert!(t
            .cookie_value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'));
        assert_eq!(t.cookie_value, Token::new("a b;c=d".into()).cookie_value);
        assert_ne!(t.cookie_value, Token::new("other".into()).cookie_value);
        assert!(t.matches_cookie(&t.cookie_value.clone()));
        assert!(!t.matches_cookie("a b;c=d"));
        // …and it is no bearer.
        assert!(!t.matches(&t.cookie_value.clone()));
    }

    #[test]
    fn bootstrap_url_encodes_the_token() {
        assert_eq!(
            bootstrap_url("http://127.0.0.1:7878", "x&y"),
            "http://127.0.0.1:7878/?token=x%26y"
        );
    }
}

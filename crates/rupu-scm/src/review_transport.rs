//! Fresh bounded review reads. No retries, redirect following, cache or vendor URL traversal.
use crate::{error::ScmError, platform::Platform};
use reqwest::header::HeaderMap;
use serde_json::Value;
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use url::Url;

pub const MAX_REVIEW_BODY: usize = 2 * 1024 * 1024;
const MAX_HEADERS: usize = 128 * 1024;
#[derive(Debug)]
pub struct ReviewResponse {
    /// GraphQL retains the full wrapper; projections must inspect errors and data.
    pub body: Value,
    pub next_page: Option<u32>,
    /// False is not evidence of exhausted provider pagination.
    pub pagination_known: bool,
    pub version: Option<String>,
    pub bytes: usize,
}
#[derive(Clone)]
pub(crate) struct ReviewTransport {
    http: reqwest_middleware::ClientWithMiddleware,
    base: Option<Url>,
    graphql: Option<Url>,
    web: Option<Url>,
    platform: Platform,
    lifetime: String,
}
pub(crate) fn validate_budget(max_bytes: usize) -> Result<(), ScmError> {
    if !(1..=MAX_REVIEW_BODY).contains(&max_bytes) {
        Err(bad("Invalid review input byte budget"))
    } else {
        Ok(())
    }
}
fn bad(message: &'static str) -> ScmError {
    ScmError::BadRequest {
        message: message.into(),
    }
}
fn conflict(message: &'static str) -> ScmError {
    ScmError::Conflict {
        message: message.into(),
    }
}
pub(crate) fn sanitized_auth(platform: Platform) -> ScmError {
    ScmError::Unauthorized {
        platform: platform.as_str().into(),
        hint: "Review authentication unavailable; refresh the owning account separately".into(),
    }
}
pub(crate) fn transport_failure() -> ScmError {
    ScmError::Network(anyhow::anyhow!("bounded review transport failed"))
}
fn clean_url(raw: &str) -> Option<Url> {
    let u = Url::parse(raw).ok()?;
    (matches!(u.scheme(), "https" | "http")
        && u.host_str().is_some()
        && u.username().is_empty()
        && u.password().is_none()
        && u.query().is_none()
        && u.fragment().is_none())
    .then_some(u)
}
fn same_authority(a: &Url, b: &Url) -> bool {
    a.scheme() == b.scheme()
        && a.host_str() == b.host_str()
        && a.port_or_known_default() == b.port_or_known_default()
        && b.username().is_empty()
        && b.password().is_none()
}
fn profile(platform: Platform, raw: &str) -> Option<(Url, Option<Url>, Url)> {
    let mut base = clean_url(raw)?;
    let path = base.path().trim_end_matches('/').to_string();
    let mut web = base.clone();
    let mut graphql = None;
    match platform {
        Platform::Github => {
            if !path.is_empty() && path != "/api/v3" {
                return None;
            }
            if base.host_str() == Some("api.github.com") {
                if base.scheme() != "https"
                    || base.port_or_known_default() != Some(443)
                    || !path.is_empty()
                {
                    return None;
                }
                web.set_host(Some("github.com")).ok()?;
            }
            web.set_path("/");
            let mut gql = base.clone();
            // Match GithubClient::graphql_url_for: a custom root is an
            // enterprise API profile, not public api.github.com's /graphql.
            gql.set_path(if base.host_str() == Some("api.github.com") {
                "/graphql"
            } else {
                "/api/graphql"
            });
            graphql = Some(gql);
        }
        Platform::Gitlab => {
            let prefix = path.strip_suffix("/api/v4")?;
            web.set_path(&format!("{prefix}/"));
        }
    }
    base.set_path(&path);
    Some((base, graphql, web))
}
impl ReviewTransport {
    pub(crate) fn new(
        platform: Platform,
        base: &str,
        timeout: Duration,
        sink: Arc<dyn rupu_netflow::FlowSink>,
    ) -> Self {
        let parsed = profile(platform, base);
        let (base, graphql, web) = match parsed {
            Some((b, g, w)) => (Some(b), g, Some(w)),
            None => (None, None, None),
        };
        let http = rupu_netflow::http::client_with(
            rupu_netflow::FlowCtx::system(rupu_netflow::Origin::Scm(platform.as_str().into())),
            reqwest::Client::builder()
                .timeout(timeout)
                // reqwest otherwise retries some protocol NACKs internally.
                // A review page budgets one wire attempt and owns retry policy.
                .retry(reqwest::retry::never())
                .http2_max_header_list_size(MAX_HEADERS as u32)
                .redirect(reqwest::redirect::Policy::none()),
            sink,
        )
        .expect("bounded review netflow client build");
        Self {
            http,
            base,
            graphql,
            web,
            platform,
            lifetime: format!("{:032x}", rand::random::<u128>()),
        }
    }
    pub(crate) fn identity_epoch(&self, credential_epoch: u64) -> String {
        format!("{}:{credential_epoch}", self.lifetime)
    }
    pub(crate) fn api_profile(&self) -> String {
        match &self.base {
            Some(base) => format!(
                "review-v1:{}:{}:{}",
                self.platform.as_str(),
                base,
                if self.platform == Platform::Github {
                    "rest-2022-11-28+graphql-review-v1"
                } else {
                    "rest-v4-review-v1"
                }
            ),
            None => "unsupported-review-api-profile".into(),
        }
    }
    pub(crate) fn matches_origin(&self, origin: &str) -> bool {
        let (Some(web), Some(origin)) = (&self.web, clean_url(origin)) else {
            return false;
        };
        if !same_authority(web, &origin) || origin.path().contains('%') {
            return false;
        }
        let Some(repo) = origin.path().strip_prefix(web.path()) else {
            return false;
        };
        let segments: Vec<_> = repo.trim_end_matches('/').split('/').collect();
        segments.len() >= 2
            && (self.platform != Platform::Github || segments.len() == 2)
            && segments.iter().all(|s| safe_segment(s))
    }
    pub(crate) fn get_request(
        &self,
        path: &str,
    ) -> Result<(reqwest_middleware::RequestBuilder, Url), ScmError> {
        let base = self
            .base
            .as_ref()
            .ok_or_else(|| bad("Unsupported review API profile"))?;
        validate_path(self.platform, path)?;
        let url = Url::parse(&format!("{}{path}", base.as_str().trim_end_matches('/')))
            .map_err(|_| bad("Invalid review API path"))?;
        if !same_authority(base, &url) {
            return Err(bad("Review API authority mismatch"));
        }
        Ok((self.headers(self.http.get(url.clone())), url))
    }
    pub(crate) fn graphql_request(
        &self,
        query: &str,
        variables: Value,
    ) -> Result<(reqwest_middleware::RequestBuilder, Url), ScmError> {
        let url = self
            .graphql
            .clone()
            .ok_or_else(|| bad("Unsupported review GraphQL profile"))?;
        let trimmed = query.trim_start();
        if query.len() > 64 * 1024
            || !trimmed.starts_with("query")
            || query
                .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                .any(|s| matches!(s, "mutation" | "subscription"))
        {
            return Err(bad("Only fixed bounded review queries are supported"));
        }
        let body = serde_json::json!({"query":query,"variables":variables});
        if serde_json::to_vec(&body)
            .map_err(|_| bad("Invalid review query"))?
            .len()
            > 128 * 1024
        {
            return Err(bad("Review query exceeds input budget"));
        }
        Ok((self.headers(self.http.post(url.clone())).json(&body), url))
    }
    fn headers(
        &self,
        request: reqwest_middleware::RequestBuilder,
    ) -> reqwest_middleware::RequestBuilder {
        let request = request
            .header(reqwest::header::USER_AGENT, "rupu/0")
            .header(reqwest::header::ACCEPT_ENCODING, "identity")
            .header(reqwest::header::CACHE_CONTROL, "no-cache")
            .header(reqwest::header::PRAGMA, "no-cache")
            .header(reqwest::header::ACCEPT, "application/json");
        if self.platform == Platform::Github {
            request.header("X-GitHub-Api-Version", "2022-11-28")
        } else {
            request
        }
    }
    pub(crate) async fn finish(
        &self,
        mut response: reqwest::Response,
        url: &Url,
        max_bytes: usize,
    ) -> Result<ReviewResponse, ScmError> {
        if response.url() != url {
            return Err(conflict("Review redirect is not permitted"));
        }
        let headers = response.headers();
        let header_bytes = headers
            .iter()
            .try_fold(0usize, |n, (k, v)| {
                n.checked_add(k.as_str().len() + v.as_bytes().len() + 4)
            })
            .ok_or_else(|| bad("Review response headers exceed budget"))?;
        if header_bytes > MAX_HEADERS {
            return Err(bad("Review response headers exceed budget"));
        }
        let status = response.status().as_u16();
        if !(200..300).contains(&status) {
            return Err(status_error(self.platform, status, headers));
        }
        if let Some(encoding) = single(headers, "content-encoding")? {
            if !encoding.eq_ignore_ascii_case("identity") {
                return Err(bad("Unsupported compressed review response"));
            }
        }
        if let Some(size) = single(headers, "content-length")? {
            if size
                .parse::<u64>()
                .map_err(|_| bad("Invalid review content length"))?
                > max_bytes as u64
            {
                return Err(bad("Review input byte limit exceeded"));
            }
        }
        let version = version(self.platform, headers)?;
        let headers = headers.clone();
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| transport_failure())? {
            if chunk.len() > max_bytes.saturating_sub(bytes.len()) {
                return Err(bad("Review input byte limit exceeded"));
            }
            bytes.extend_from_slice(&chunk);
        }
        let body: Value = serde_json::from_slice(&bytes)
            .map_err(|_| bad("Invalid bounded review JSON response"))?;
        let (next_page, pagination_known) = pagination(url, &headers)?;
        // A full numbered page without pagination evidence cannot be called
        // exhausted. The caller never receives an ambiguous successful end.
        if !pagination_known {
            let query = query_map(url)?;
            if let Some(per_page) = query.get("per_page") {
                let per_page = positive(per_page)? as usize;
                if body.as_array().is_some_and(|rows| rows.len() >= per_page) {
                    return Err(bad(
                        "Review pagination metadata is unavailable for a full page",
                    ));
                }
            }
        }
        Ok(ReviewResponse {
            body,
            next_page,
            pagination_known,
            version,
            bytes: bytes.len(),
        })
    }
}
fn safe_segment(s: &str) -> bool {
    if s.is_empty() || s.len() > 1024 {
        return false;
    }
    // Only namespace slashes may be percent encoded. Never admit encoded dot
    // segments, double encoding, credentials, delimiters or backslashes.
    let mut decoded = String::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c == b'%' {
            if bytes.get(i + 1) != Some(&b'2') || !matches!(bytes.get(i + 2), Some(b'f' | b'F')) {
                return false;
            }
            decoded.push('/');
            i += 3;
        } else if c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.' | b'~') {
            decoded.push(c as char);
            i += 1;
        } else {
            return false;
        }
    }
    decoded
        .split('/')
        .all(|part| !part.is_empty() && part != "." && part != "..")
}
fn positive(s: &str) -> Result<u32, ScmError> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return Err(bad("Invalid review page number"));
    }
    s.parse::<u32>()
        .ok()
        .filter(|n| *n > 0)
        .ok_or_else(|| bad("Invalid review page number"))
}
fn validate_path(platform: Platform, path: &str) -> Result<(), ScmError> {
    if path.len() > 8192
        || !path.starts_with('/')
        || path.starts_with("//")
        || path.contains('#')
        || path.contains('\\')
    {
        return Err(bad("Invalid fixed review API path"));
    }
    let raw = path.split('?').next().unwrap_or_default();
    let parts: Vec<_> = raw.trim_start_matches('/').split('/').collect();
    if !parts.iter().enumerate().all(|(index, s)| {
        safe_segment(s) && (!s.contains('%') || (platform == Platform::Gitlab && index == 1))
    }) {
        return Err(bad("Invalid fixed review API path"));
    }
    let numeric = |s: &&str| positive(s).is_ok();
    let allowed = match platform {
        Platform::Github => {
            parts.len() >= 5
                && parts[0] == "repos"
                && parts[3] == "pulls"
                && numeric(&parts[4])
                && (parts.len() == 5 || (parts.len() == 6 && parts[5] == "files"))
        }
        Platform::Gitlab => {
            parts.len() >= 2
                && parts[0] == "projects"
                && (parts.len() == 2
                    || (parts.len() == 5
                        && parts[2] == "pipelines"
                        && numeric(&parts[3])
                        && parts[4] == "jobs")
                    || (parts.len() >= 4
                        && parts[2] == "merge_requests"
                        && numeric(&parts[3])
                        && (parts.len() == 4
                            || (parts.len() == 5
                                && matches!(parts[4], "discussions" | "diffs" | "versions"))
                            || (parts.len() == 6
                                && matches!(parts[4], "discussions" | "versions")))))
        }
    };
    if !allowed {
        return Err(bad("Unsupported fixed review API path"));
    }
    let url = Url::parse(&format!("https://review.invalid{path}"))
        .map_err(|_| bad("Invalid review query"))?;
    let query = query_map(&url)?;
    for (k, v) in query {
        match k.as_str() {
            "page" if positive(&v)? <= 512 => (),
            "per_page" if positive(&v)? <= 100 => (),
            "include_retried"
                if platform == Platform::Gitlab
                    && parts.get(4) == Some(&"jobs")
                    && v == "false" =>
            {}
            _ => return Err(bad("Unsupported review query parameter")),
        }
    }
    Ok(())
}
fn single<'a>(headers: &'a HeaderMap, name: &str) -> Result<Option<&'a str>, ScmError> {
    let mut values = headers.get_all(name).iter();
    let first = values.next();
    if values.next().is_some() {
        return Err(bad("Ambiguous review response header"));
    }
    first
        .map(|h| {
            h.to_str()
                .map_err(|_| bad("Invalid review response header"))
        })
        .transpose()
}
fn query_map(url: &Url) -> Result<BTreeMap<String, String>, ScmError> {
    let mut map = BTreeMap::new();
    for (k, v) in url.query_pairs() {
        if map.insert(k.into_owned(), v.into_owned()).is_some() {
            return Err(bad("Ambiguous review pagination query"));
        }
    }
    Ok(map)
}
fn pagination(url: &Url, headers: &HeaderMap) -> Result<(Option<u32>, bool), ScmError> {
    let expected = query_map(url)?;
    let current = expected
        .get("page")
        .map(|v| positive(v))
        .transpose()?
        .unwrap_or(1);
    if let Some(reported) = single(headers, "x-page")? {
        if positive(reported)? != current {
            return Err(conflict("Review provider page differs from requested page"));
        }
    }
    if let Some(reported) = single(headers, "x-per-page")? {
        if expected.get("per_page").map(|v| positive(v)).transpose()? != Some(positive(reported)?) {
            return Err(conflict("Review provider page size differs from request"));
        }
    }
    let mut expected_without_page = expected;
    expected_without_page.remove("page");
    let mut next = None;
    let mut known = false;
    let mut seen_next = false;
    for value in headers.get_all("link").iter() {
        known = true;
        let value = value
            .to_str()
            .map_err(|_| bad("Invalid review pagination link"))?;
        if value.trim().is_empty() {
            return Err(bad("Empty review pagination link"));
        }
        for link in value.split(',') {
            let (target, params) = link
                .trim()
                .split_once('>')
                .ok_or_else(|| bad("Malformed review pagination link"))?;
            let target = target
                .strip_prefix('<')
                .ok_or_else(|| bad("Malformed review pagination link"))?;
            let linked = Url::parse(target).map_err(|_| bad("Invalid review pagination URL"))?;
            if !same_authority(url, &linked)
                || linked.path() != url.path()
                || linked.fragment().is_some()
            {
                return Err(conflict("Review pagination scope mismatch"));
            }
            let mut actual = query_map(&linked)?;
            let page = actual
                .remove("page")
                .ok_or_else(|| bad("Review pagination has no page"))?;
            let page = positive(&page)?;
            if actual != expected_without_page {
                return Err(conflict("Review pagination filter mismatch"));
            }
            let relations: Vec<_> = params
                .split(';')
                .map(str::trim)
                .filter_map(|p| p.strip_prefix("rel="))
                .collect();
            if relations.len() != 1 {
                return Err(bad("Ambiguous review pagination relation"));
            }
            let rel = relations[0].trim_matches('"');
            if !matches!(rel, "next" | "prev" | "first" | "last") {
                return Err(bad("Unsupported review pagination relation"));
            }
            if rel == "next" {
                if seen_next || page != current.saturating_add(1) || page > 512 {
                    return Err(conflict("Review pagination is not one forward page"));
                }
                seen_next = true;
                next = Some(page);
            }
        }
    }
    if let Some(raw) = single(headers, "x-next-page")? {
        let from_header = if raw.is_empty() {
            None
        } else {
            Some(positive(raw)?)
        };
        if from_header.is_some_and(|n| n != current.saturating_add(1) || n > 512)
            || (known && from_header != next)
        {
            return Err(conflict("Review pagination headers disagree"));
        }
        known = true;
        next = from_header;
    }
    Ok((next, known))
}
fn version(platform: Platform, headers: &HeaderMap) -> Result<Option<String>, ScmError> {
    let names: &[&str] = if platform == Platform::Github {
        &[
            "x-github-api-version-selected",
            "x-github-enterprise-version",
        ]
    } else {
        &["x-gitlab-version"]
    };
    let mut parts = Vec::new();
    for name in names {
        if let Some(v) = single(headers, name)? {
            if v.is_empty()
                || v.len() > 128
                || !v
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_' | b'+'))
            {
                return Err(bad("Invalid review version marker"));
            }
            parts.push(format!("{name}:{v}"));
        }
    }
    Ok((!parts.is_empty()).then(|| parts.join(";")))
}
fn status_error(platform: Platform, status: u16, headers: &HeaderMap) -> ScmError {
    match status {
        401 => sanitized_auth(platform),
        403 => ScmError::Forbidden {
            platform: platform.as_str().into(),
            message: "Review read was refused".into(),
        },
        404 => ScmError::NotFound {
            what: "Selected review resource".into(),
        },
        429 => ScmError::RateLimited {
            retry_after: single(headers, "retry-after")
                .ok()
                .flatten()
                .and_then(|s| s.parse::<u64>().ok())
                .filter(|n| *n <= 86400)
                .map(Duration::from_secs),
        },
        500..=599 => ScmError::Transient(anyhow::anyhow!("Review provider unavailable")),
        300..=399 => conflict("Review redirects are refused"),
        _ => bad("Review provider returned an unsupported status"),
    }
}

#[cfg(test)]
mod tests;

use super::*;
use reqwest::{
    header::{HeaderMap, HeaderValue, ACCEPT_ENCODING},
    Method,
};
use serde_json::json;
use std::sync::Arc;
use url::Url;

const HEADERS: usize = 128 * 1024;
pub(crate) fn http(
    provider: &str,
    sink: Arc<dyn rupu_netflow::FlowSink>,
) -> anyhow::Result<reqwest_middleware::ClientWithMiddleware> {
    Ok(rupu_netflow::http::client_with(
        rupu_netflow::FlowCtx::system(rupu_netflow::Origin::Scm(provider.into())),
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .timeout(std::time::Duration::from_secs(10))
            .http2_max_header_list_size(HEADERS as u32)
            .user_agent("rupu-operator-discussion"),
        sink,
    )?)
}
pub(crate) struct Session {
    pub provider: IssueTracker,
    pub origin: String,
    graphql: String,
    pub epoch: String,
    client: reqwest_middleware::ClientWithMiddleware,
    auth: HeaderMap,
    current: Arc<dyn Fn() -> String + Send + Sync>,
}
pub(crate) struct Response {
    pub value: Value,
    pub headers: HeaderMap,
    pub bytes: usize,
}
impl Session {
    pub(crate) fn new(
        provider: IssueTracker,
        origin: String,
        graphql: String,
        epoch: String,
        client: reqwest_middleware::ClientWithMiddleware,
        auth: HeaderMap,
        current: Arc<dyn Fn() -> String + Send + Sync>,
    ) -> Result<Self, ScmError> {
        if origin.len() > 2048
            || graphql.len() > 2048
            || crate::operator_issues::safe_origin(&origin).as_deref() != Some(origin.as_str())
            || !valid(&epoch, 256)
        {
            return Err(unsupported());
        }
        if !graphql.is_empty() {
            let a = Url::parse(&origin).map_err(|_| unsupported())?;
            let b = Url::parse(&graphql).map_err(|_| unsupported())?;
            if crate::operator_issues::safe_origin(&graphql).as_deref() != Some(graphql.as_str())
                || a.origin() != b.origin()
            {
                return Err(unsupported());
            }
        }
        Ok(Self {
            provider,
            origin,
            graphql,
            epoch,
            client,
            auth,
            current,
        })
    }
    pub fn check(&self) -> Result<(), ScmError> {
        if (self.current)() == self.epoch {
            Ok(())
        } else {
            Err(changed())
        }
    }
    pub fn identity(
        &self,
        project_id: String,
        issue_id: String,
        identifier: String,
    ) -> DiscussionIdentity {
        DiscussionIdentity {
            connector_epoch: self.epoch.clone(),
            api_profile: format!(
                "discussion-v1:{:?}:{}:{}",
                self.provider, self.origin, self.graphql
            ),
            project_id,
            issue_id,
            identifier,
        }
    }
    pub async fn get(&self, path: &str, cap: usize) -> Result<Response, ScmError> {
        if !path.starts_with('/')
            || path.starts_with("//")
            || path
                .split('?')
                .next()
                .unwrap_or(path)
                .split('/')
                .any(|part| matches!(part, "." | ".."))
            || path.chars().any(char::is_control)
            || path.contains('#')
        {
            return Err(failed());
        }
        self.read(Method::GET, format!("{}{path}", self.origin), None, cap)
            .await
    }
    pub async fn graphql(
        &self,
        query: &str,
        variables: Value,
        cap: usize,
    ) -> Result<Response, ScmError> {
        if self.graphql.is_empty() {
            return Err(unsupported());
        }
        let mut r = self
            .read(
                Method::POST,
                self.graphql.clone(),
                Some(json!({"query":query,"variables":variables})),
                cap,
            )
            .await?;
        if r.value
            .get("errors")
            .is_some_and(|e| !e.is_null() && e.as_array().is_none_or(|a| !a.is_empty()))
        {
            return Err(failed());
        }
        r.value = r
            .value
            .get("data")
            .filter(|v| v.is_object())
            .cloned()
            .ok_or_else(failed)?;
        Ok(r)
    }
    async fn read(
        &self,
        method: Method,
        url: String,
        body: Option<Value>,
        cap: usize,
    ) -> Result<Response, ScmError> {
        self.check()?;
        if cap == 0 || cap > MAX_PAGE_BYTES {
            return Err(failed());
        }
        let mut request = self
            .client
            .request(method, url)
            .headers(self.auth.clone())
            .header(ACCEPT_ENCODING, HeaderValue::from_static("identity"));
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.map_err(|_| failed())?;
        let result = bounded(response, cap).await?;
        self.check()?;
        Ok(result)
    }
}
pub(super) async fn bounded(
    mut response: reqwest::Response,
    cap: usize,
) -> Result<Response, ScmError> {
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    if headers
        .iter()
        .map(|(k, v)| k.as_str().len() + v.as_bytes().len() + 4)
        .sum::<usize>()
        > HEADERS
        || response.content_length().is_some_and(|n| n > cap as u64)
    {
        return Err(failed());
    }
    if headers
        .get(reqwest::header::CONTENT_ENCODING)
        .is_some_and(|v| v.as_bytes() != b"identity")
    {
        return Err(failed());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| failed())? {
        if chunk.len() > cap.saturating_sub(bytes.len()) {
            return Err(failed());
        }
        bytes.extend_from_slice(&chunk);
    }
    match status {
        200..=299 => {}
        401 | 403 => {
            return Err(ScmError::Forbidden {
                platform: "issue".into(),
                message: "The selected issue account refused this read".into(),
            })
        }
        404 => {
            return Err(ScmError::NotFound {
                what: "issue discussion".into(),
            })
        }
        429 => return Err(ScmError::RateLimited { retry_after: None }),
        _ => return Err(failed()),
    }
    let value = serde_json::from_slice(&bytes).map_err(|_| failed())?;
    Ok(Response {
        value,
        headers,
        bytes: bytes.len(),
    })
}
/// Validate Link/X-Next-Page but never follow URLs returned by a provider.
pub(crate) fn rest_next(
    headers: &HeaderMap,
    origin: &str,
    path: &str,
    current: u32,
    count: usize,
    require_full_header: bool,
) -> Result<Option<String>, ScmError> {
    let expected = Url::parse(&format!("{origin}{path}")).map_err(|_| failed())?;
    let mut next: Option<u32> = None;
    let mut known = false;
    let values = headers.get_all("x-next-page").iter().collect::<Vec<_>>();
    if values.len() > 1 {
        return Err(failed());
    }
    if let Some(v) = values.first() {
        known = true;
        let s = v.to_str().map_err(|_| failed())?;
        if !s.is_empty() {
            let n = page_number(Some(s), 1)?;
            if n != current.checked_add(1).ok_or_else(failed)? {
                return Err(failed());
            }
            next = Some(n);
        }
    }
    for value in headers.get_all(reqwest::header::LINK).iter() {
        let raw = value.to_str().map_err(|_| failed())?;
        for entry in raw.split(',') {
            let mut parts = entry.trim().split(';');
            let link = parts.next().ok_or_else(failed)?;
            if !link.starts_with('<') || !link.ends_with('>') {
                return Err(failed());
            }
            let url = Url::parse(&link[1..link.len() - 1]).map_err(|_| failed())?;
            if url.origin() != expected.origin()
                || url.path() != expected.path()
                || !url.username().is_empty()
                || url.password().is_some()
                || url.fragment().is_some()
            {
                return Err(failed());
            }
            let scope =
                |url: &Url| -> Result<std::collections::BTreeMap<String, String>, ScmError> {
                    let mut out = std::collections::BTreeMap::new();
                    for (k, v) in url.query_pairs() {
                        if k != "page" && out.insert(k.into_owned(), v.into_owned()).is_some() {
                            return Err(failed());
                        }
                    }
                    Ok(out)
                };
            if scope(&url)? != scope(&expected)? {
                return Err(failed());
            }
            let page: Vec<_> = url.query_pairs().filter(|(k, _)| k == "page").collect();
            if page.len() != 1 {
                return Err(failed());
            }
            let p = page_number(Some(&page[0].1), 1)?;
            let rels: Vec<_> = parts
                .filter_map(|s| s.trim().strip_prefix("rel="))
                .collect();
            if rels.len() != 1 {
                return Err(failed());
            }
            if rels[0]
                .trim_matches('"')
                .split_whitespace()
                .any(|r| r == "next")
            {
                if p != current.checked_add(1).ok_or_else(failed)?
                    || next.is_some_and(|n| n != p)
                    || (known && next.is_none())
                {
                    return Err(failed());
                }
                next = Some(p);
                known = true;
            }
        }
    }
    if count < PAGE_SIZE {
        if next.is_some() {
            return Ok(next.map(|n| n.to_string()));
        }
        return Ok(None);
    }
    if !known && require_full_header {
        return Err(failed());
    }
    if !known {
        let following = current
            .checked_add(1)
            .filter(|n| *n <= 1_000_000)
            .ok_or_else(failed)?;
        return Ok(Some(following.to_string()));
    }
    Ok(next.map(|n| n.to_string()))
}

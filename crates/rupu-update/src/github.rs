use crate::model::{parse_releases, Release, ReleaseSource};

const API: &str = "https://api.github.com";
const MAX_BYTES: u64 = 200 * 1024 * 1024;

pub fn releases_api_url(owner_repo: &str) -> String {
    format!("{API}/repos/{owner_repo}/releases?per_page=100")
}

pub struct GithubReleaseSource {
    owner_repo: String,
    api_base: String,
    client: reqwest_middleware::ClientWithMiddleware,
}

impl GithubReleaseSource {
    pub fn new(owner_repo: impl Into<String>) -> Self {
        Self::with_api_base(owner_repo, API)
    }

    /// As [`Self::new`], with the GitHub API base overridden. Exists so
    /// tests can point at a mock server instead of `api.github.com`.
    pub fn with_api_base(owner_repo: impl Into<String>, api_base: impl Into<String>) -> Self {
        Self {
            owner_repo: owner_repo.into(),
            api_base: api_base.into(),
            // Self-update egress, explicitly out of netflow's scope (it is
            // not inference-driven traffic) — a caller-constructed
            // `NullSink`, not a stopgap. Infallible constructor (`-> Self`);
            // `.expect()` preserves the deleted `http::client()` fallback's
            // panic-on-failure behaviour. No fallback to an uninstrumented
            // client.
            client: rupu_netflow::http::client_with(
                rupu_netflow::FlowCtx::system(rupu_netflow::Origin::Update),
                reqwest::Client::builder(),
                std::sync::Arc::new(rupu_netflow::NullSink),
            )
            .expect("update client build"),
        }
    }
}

fn req(
    client: &reqwest_middleware::ClientWithMiddleware,
    url: &str,
) -> reqwest_middleware::RequestBuilder {
    let mut b = client.get(url).header("User-Agent", "rupu-update");
    if let Ok(tok) = std::env::var("GITHUB_TOKEN") {
        if !tok.is_empty() {
            b = b.header("Authorization", format!("Bearer {tok}"));
        }
    }
    b
}

#[async_trait::async_trait]
impl ReleaseSource for GithubReleaseSource {
    async fn list_releases(&self) -> Result<Vec<Release>, crate::UpdateError> {
        let url = format!(
            "{}/repos/{}/releases?per_page=100",
            self.api_base, self.owner_repo
        );
        let resp = req(&self.client, &url)
            .send()
            .await
            .map_err(|e| crate::UpdateError::Network(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(crate::UpdateError::Network(format!(
                "GitHub API {}",
                resp.status()
            )));
        }
        let body = resp
            .text()
            .await
            .map_err(|e| crate::UpdateError::Network(e.to_string()))?;
        parse_releases(&body)
    }
}

/// Download `url` into memory (UA header, optional token, size cap + timeout).
pub async fn download_bytes(url: &str) -> Result<Vec<u8>, crate::UpdateError> {
    download_bytes_with_progress(url, |_, _| {}).await
}

/// Download `url` into memory, invoking `on_progress(downloaded, total)` as
/// bytes arrive so callers can render a progress bar. `total` is the
/// server-reported content length when the response advertises one (`None`
/// otherwise). Same UA/token/size-cap/timeout semantics as [`download_bytes`];
/// the cap is enforced against the running total while streaming rather than
/// only after the whole body lands.
pub async fn download_bytes_with_progress(
    url: &str,
    mut on_progress: impl FnMut(u64, Option<u64>),
) -> Result<Vec<u8>, crate::UpdateError> {
    // Self-update egress, explicitly out of netflow's scope (it is not
    // inference-driven traffic) — a caller-constructed `NullSink`, not a
    // stopgap. Already fallible (`-> Result<_, UpdateError>`), so the
    // client build propagates via `?` too.
    let client = rupu_netflow::http::client_with(
        rupu_netflow::FlowCtx::system(rupu_netflow::Origin::Update),
        reqwest::Client::builder().timeout(std::time::Duration::from_secs(60)),
        std::sync::Arc::new(rupu_netflow::NullSink),
    )
    .map_err(|e| crate::UpdateError::Network(e.to_string()))?;
    let mut resp = req(&client, url)
        .send()
        .await
        .map_err(|e| crate::UpdateError::Network(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(crate::UpdateError::Network(format!(
            "download {}: {}",
            url,
            resp.status()
        )));
    }
    let total = resp.content_length();
    if let Some(len) = total {
        if len > MAX_BYTES {
            return Err(crate::UpdateError::Network(format!(
                "asset too large: {len} bytes"
            )));
        }
    }
    let mut buf: Vec<u8> = Vec::with_capacity(total.unwrap_or(0) as usize);
    on_progress(0, total);
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| crate::UpdateError::Network(e.to_string()))?
    {
        buf.extend_from_slice(&chunk);
        if buf.len() as u64 > MAX_BYTES {
            return Err(crate::UpdateError::Network(
                "asset exceeded size cap".into(),
            ));
        }
        on_progress(buf.len() as u64, total);
    }
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn builds_releases_url() {
        assert_eq!(
            releases_api_url("Section9Labs/rupu"),
            "https://api.github.com/repos/Section9Labs/rupu/releases?per_page=100"
        );
    }
}

use serde::Deserialize;

/// Release channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    Stable,
    Beta,
}

impl std::str::FromStr for Channel {
    type Err = crate::UpdateError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "stable" => Ok(Channel::Stable),
            "beta" => Ok(Channel::Beta),
            other => Err(crate::UpdateError::BadChannel(other.to_string())),
        }
    }
}

impl std::fmt::Display for Channel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Channel::Stable => "stable",
            Channel::Beta => "beta",
        })
    }
}

impl Channel {
    /// Rolling tag name for this channel.
    pub fn rolling_tag(&self) -> &'static str {
        match self {
            Channel::Stable => "latest-stable",
            Channel::Beta => "latest-beta",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    pub url: String,
}

#[derive(Debug, Clone)]
pub struct Release {
    pub tag: String,
    pub version: semver::Version,
    pub prerelease: bool,
    pub assets: Vec<Asset>,
}

// Raw GitHub shapes (subset).
#[derive(Deserialize)]
struct RawAsset {
    name: String,
    browser_download_url: String,
}
#[derive(Deserialize)]
struct RawRelease {
    tag_name: String,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<RawAsset>,
}

/// Parse the GitHub `/releases` JSON. Releases whose tag is not `v<semver>`
/// are skipped (tolerant — the repo may carry unrelated tags like `latest-*`).
pub fn parse_releases(json: &str) -> Result<Vec<Release>, crate::UpdateError> {
    let raw: Vec<RawRelease> =
        serde_json::from_str(json).map_err(|e| crate::UpdateError::Parse(e.to_string()))?;
    let mut out = Vec::new();
    for r in raw {
        let Some(ver_str) = r.tag_name.strip_prefix('v') else {
            continue;
        };
        let Ok(version) = semver::Version::parse(ver_str) else {
            continue;
        };
        out.push(Release {
            tag: r.tag_name,
            version,
            prerelease: r.prerelease,
            assets: r
                .assets
                .into_iter()
                .map(|a| Asset {
                    name: a.name,
                    url: a.browser_download_url,
                })
                .collect(),
        });
    }
    Ok(out)
}

/// Port: something that can list this repo's releases.
#[async_trait::async_trait]
pub trait ReleaseSource: Send + Sync {
    async fn list_releases(&self) -> Result<Vec<Release>, crate::UpdateError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"[
        {"tag_name":"latest-beta","prerelease":true,"assets":[]},
        {"tag_name":"v0.35.4-beta","prerelease":true,
         "assets":[{"name":"rupu-darwin-arm64","browser_download_url":"https://x/b"},
                   {"name":"rupu-darwin-arm64.sha256","browser_download_url":"https://x/b.sha"}]},
        {"tag_name":"v0.35.3","prerelease":false,
         "assets":[{"name":"rupu-darwin-arm64","browser_download_url":"https://x/s"}]}
    ]"#;

    #[test]
    fn parses_and_skips_non_semver_tags() {
        let rs = parse_releases(FIXTURE).unwrap();
        assert_eq!(rs.len(), 2, "latest-beta skipped");
        assert!(rs
            .iter()
            .any(|r| r.version.to_string() == "0.35.4-beta" && r.prerelease));
        assert!(rs
            .iter()
            .any(|r| r.version.to_string() == "0.35.3" && !r.prerelease));
    }

    #[test]
    fn channel_from_str_and_rolling_tag() {
        use std::str::FromStr;
        assert_eq!(Channel::from_str("BETA").unwrap(), Channel::Beta);
        assert_eq!(Channel::Stable.rolling_tag(), "latest-stable");
        assert!(Channel::from_str("nightly").is_err());
    }
}

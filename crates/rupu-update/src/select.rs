use crate::model::{Asset, Channel, Release};

/// The binary + `.sha256` assets for `platform` (e.g. "darwin-arm64"), if both present.
pub fn asset_for<'a>(release: &'a Release, platform: &str) -> Option<(&'a Asset, &'a Asset)> {
    let bin_name = format!("rupu-{platform}");
    let sha_name = format!("rupu-{platform}.sha256");
    let bin = release.assets.iter().find(|a| a.name == bin_name)?;
    let sha = release.assets.iter().find(|a| a.name == sha_name)?;
    Some((bin, sha))
}

/// Highest-semver release for the channel that also carries the platform asset.
/// - Stable: only full releases (`!prerelease`).
/// - Beta: any release (prerelease or full) — semver precedence means a promoted
///   stable (`0.35.4`) outranks its beta (`0.35.4-beta`).
pub fn select_latest<'a>(
    releases: &'a [Release],
    channel: Channel,
    platform: &str,
) -> Option<&'a Release> {
    releases
        .iter()
        .filter(|r| match channel {
            Channel::Stable => !r.prerelease,
            Channel::Beta => true,
        })
        .filter(|r| asset_for(r, platform).is_some())
        .max_by(|a, b| a.version.cmp(&b.version))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_releases;

    const F: &str = r#"[
        {"tag_name":"v0.35.5-beta","prerelease":true,
         "assets":[{"name":"rupu-darwin-arm64","browser_download_url":"u"},
                   {"name":"rupu-darwin-arm64.sha256","browser_download_url":"u"}]},
        {"tag_name":"v0.35.4","prerelease":false,
         "assets":[{"name":"rupu-darwin-arm64","browser_download_url":"u"},
                   {"name":"rupu-darwin-arm64.sha256","browser_download_url":"u"}]},
        {"tag_name":"v0.35.4-beta","prerelease":true,
         "assets":[{"name":"rupu-darwin-arm64","browser_download_url":"u"},
                   {"name":"rupu-darwin-arm64.sha256","browser_download_url":"u"}]}
    ]"#;

    #[test]
    fn stable_picks_highest_full_release() {
        let rs = parse_releases(F).unwrap();
        let r = select_latest(&rs, Channel::Stable, "darwin-arm64").unwrap();
        assert_eq!(r.version.to_string(), "0.35.4");
    }

    #[test]
    fn beta_picks_highest_including_prereleases() {
        let rs = parse_releases(F).unwrap();
        let r = select_latest(&rs, Channel::Beta, "darwin-arm64").unwrap();
        assert_eq!(r.version.to_string(), "0.35.5-beta");
    }

    #[test]
    fn beta_prefers_promoted_stable_over_its_beta() {
        // Only 0.35.4 and 0.35.4-beta present → beta channel takes the stable.
        let rs: Vec<_> = parse_releases(F)
            .unwrap()
            .into_iter()
            .filter(|r| !r.version.to_string().starts_with("0.35.5"))
            .collect();
        let r = select_latest(&rs, Channel::Beta, "darwin-arm64").unwrap();
        assert_eq!(r.version.to_string(), "0.35.4");
    }

    #[test]
    fn none_when_platform_missing() {
        let rs = parse_releases(F).unwrap();
        assert!(select_latest(&rs, Channel::Stable, "linux-x64").is_none());
    }
}

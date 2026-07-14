use crate::decide::{decide, is_dev_exe, Decision};
use crate::install as install_mod;
use crate::model::{Channel, ReleaseSource};
use crate::select::{asset_for, select_latest};
use crate::verify::{verify_checksum, BinaryCheck};
use semver::Version;
use std::path::{Path, PathBuf};

pub struct UpdateContext {
    pub current_version: Version,
    pub channel: Channel,
    pub exe_path: PathBuf,
    pub is_dev: bool,
}

impl UpdateContext {
    pub fn from_env(
        current_version: &str,
        channel: Channel,
        exe_path: PathBuf,
    ) -> Result<Self, crate::UpdateError> {
        let cv = Version::parse(current_version)
            .map_err(|e| crate::UpdateError::Parse(e.to_string()))?;
        let is_dev = is_dev_exe(&exe_path);
        Ok(Self {
            current_version: cv,
            channel,
            exe_path,
            is_dev,
        })
    }
}

pub struct CheckOutcome {
    pub decision: Decision,
    pub latest: Option<Version>,
    pub download: Option<(String, String)>,
}

pub trait ApplyStrategy {
    fn apply(&self, verified: &[u8], target: &Path) -> Result<(), crate::UpdateError>;
}

/// Non-elevated apply: swap in place with a backup.
pub struct DirectApply;
impl ApplyStrategy for DirectApply {
    fn apply(&self, verified: &[u8], target: &Path) -> Result<(), crate::UpdateError> {
        let bak = install_mod::backup_dir().join(format!(
            "rupu-{}",
            target
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("prev")
        ));
        install_mod::swap_in_place(verified, target, Some(&bak))
    }
}

fn platform() -> String {
    crate::decide::current_platform()
}

pub async fn check(
    src: &dyn ReleaseSource,
    ctx: &UpdateContext,
) -> Result<CheckOutcome, crate::UpdateError> {
    let releases = src.list_releases().await?;
    let plat = platform();
    let Some(latest) = select_latest(&releases, ctx.channel, &plat) else {
        return Err(crate::UpdateError::NoAssetForPlatform {
            channel: ctx.channel.to_string(),
            platform: plat,
        });
    };
    let (bin, sha) = asset_for(latest, &plat).expect("select guarantees asset");
    let decision = decide(&ctx.current_version, &latest.version, false);
    Ok(CheckOutcome {
        decision,
        latest: Some(latest.version.clone()),
        download: Some((bin.url.clone(), sha.url.clone())),
    })
}

pub async fn install(
    src: &dyn ReleaseSource,
    ctx: &UpdateContext,
    force: bool,
    apply: &dyn ApplyStrategy,
    check: &dyn BinaryCheck,
    download: impl Fn(
        String,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Vec<u8>, crate::UpdateError>> + Send>,
    >,
) -> Result<Version, crate::UpdateError> {
    if ctx.is_dev {
        return Err(crate::UpdateError::DevBuild(
            ctx.exe_path.display().to_string(),
        ));
    }
    let releases = src.list_releases().await?;
    let plat = platform();
    let latest = select_latest(&releases, ctx.channel, &plat).ok_or_else(|| {
        crate::UpdateError::NoAssetForPlatform {
            channel: ctx.channel.to_string(),
            platform: plat.clone(),
        }
    })?;
    if let Decision::UpToDate | Decision::Ahead =
        decide(&ctx.current_version, &latest.version, force)
    {
        return Ok(ctx.current_version.clone());
    }
    let (bin, sha) = asset_for(latest, &plat).expect("select guarantees asset");
    let bin_bytes = download(bin.url.clone()).await?;
    let sha_text = String::from_utf8_lossy(&download(sha.url.clone()).await?).into_owned();
    verify_checksum(&bin_bytes, &sha_text)?;
    check.verify(&bin_bytes)?;
    apply.apply(&bin_bytes, &ctx.exe_path)?;
    Ok(latest.version.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Release;
    use crate::parse_releases;
    use crate::verify::NoopBinaryCheck;
    use std::sync::Mutex;

    struct MockSrc(Vec<Release>);
    #[async_trait::async_trait]
    impl ReleaseSource for MockSrc {
        async fn list_releases(&self) -> Result<Vec<Release>, crate::UpdateError> {
            Ok(self.0.clone())
        }
    }
    struct CapApply(Mutex<Option<Vec<u8>>>);
    impl ApplyStrategy for CapApply {
        fn apply(&self, verified: &[u8], _t: &Path) -> Result<(), crate::UpdateError> {
            *self.0.lock().unwrap() = Some(verified.to_vec());
            Ok(())
        }
    }

    /// Releases fixture carrying the binary + sha256 sidecar assets for `plat`.
    /// The sidecar's checksum *content* is supplied by the test's download
    /// closure (below), not by this helper — it only needs to publish the
    /// asset names/URLs so `select_latest`/`asset_for` can find them.
    fn releases_for(plat: &str) -> Vec<Release> {
        parse_releases(&format!(
            r#"[
          {{"tag_name":"v0.35.4-beta","prerelease":true,
            "assets":[{{"name":"rupu-{plat}","browser_download_url":"BIN"}},
                      {{"name":"rupu-{plat}.sha256","browser_download_url":"SHA"}}]}}
        ]"#
        ))
        .unwrap()
    }

    #[tokio::test]
    async fn install_downloads_verifies_and_applies() {
        let plat = crate::decide::current_platform();
        let payload = b"NEWBIN".to_vec();
        let sidecar = format!("{}  rupu-{plat}", crate::verify::sha256_hex(&payload));
        let src = MockSrc(releases_for(&plat));
        let ctx = UpdateContext::from_env(
            "0.35.3",
            Channel::Beta,
            PathBuf::from("/usr/local/bin/rupu"),
        )
        .unwrap();
        let cap = CapApply(Mutex::new(None));
        let payload2 = payload.clone();
        let sidecar2 = sidecar.clone();
        let dl = move |url: String| {
            let payload = payload2.clone();
            let sidecar = sidecar2.clone();
            Box::pin(async move {
                Ok(if url == "BIN" {
                    payload
                } else {
                    sidecar.into_bytes()
                })
            })
                as std::pin::Pin<
                    Box<
                        dyn std::future::Future<Output = Result<Vec<u8>, crate::UpdateError>>
                            + Send,
                    >,
                >
        };
        let v = install(&src, &ctx, false, &cap, &NoopBinaryCheck, dl)
            .await
            .unwrap();
        assert_eq!(v.to_string(), "0.35.4-beta");
        assert_eq!(cap.0.lock().unwrap().as_deref(), Some(&b"NEWBIN"[..]));
    }

    #[tokio::test]
    async fn install_refuses_dev_build() {
        let src = MockSrc(vec![]);
        let ctx = UpdateContext::from_env(
            "0.35.3",
            Channel::Beta,
            PathBuf::from("/x/target/release/rupu"),
        )
        .unwrap();
        let dl = |_u: String| {
            Box::pin(async { Ok(vec![]) })
                as std::pin::Pin<
                    Box<
                        dyn std::future::Future<Output = Result<Vec<u8>, crate::UpdateError>>
                            + Send,
                    >,
                >
        };
        assert!(matches!(
            install(&src, &ctx, false, &DirectApply, &NoopBinaryCheck, dl).await,
            Err(crate::UpdateError::DevBuild(_))
        ));
    }
}

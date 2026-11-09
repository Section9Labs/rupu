//! Private, explicit process handoff for the native desktop client.
//!
//! A receipt identifies a bound launch, not HTTP readiness or permission to
//! signal a process. The client must authenticate and verify its owned PID.

use anyhow::{bail, ensure, Result};
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::PathBuf,
};

pub(super) fn prepare(
    bind: SocketAddr,
    token_file: Option<PathBuf>,
    ready_file: Option<PathBuf>,
    launch_id: Option<String>,
) -> Result<Option<Bootstrap>> {
    let (token_file, ready_file, launch_id) = match (token_file, ready_file, launch_id) {
        (None, None, None) => return Ok(None),
        (Some(token), Some(ready), Some(id)) => (token, ready, id),
        _ => bail!("--token-file, --ready-file and --launch-id must be supplied together"),
    };
    ensure!(
        bind.ip() == IpAddr::V4(Ipv4Addr::LOCALHOST),
        "managed launches must bind to 127.0.0.1"
    );
    ensure!(valid_launch_id(&launch_id), "launch ID must be a UUID");
    Bootstrap::open(token_file, ready_file, launch_id).map(Some)
}

fn valid_launch_id(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| {
            if [8, 13, 18, 23].contains(&index) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}

#[cfg(unix)]
pub(super) use unix::Bootstrap;

#[cfg(not(unix))]
pub(super) struct Bootstrap;
#[cfg(not(unix))]
impl Bootstrap {
    fn open(_: PathBuf, _: PathBuf, _: String) -> Result<Self> {
        bail!("managed desktop startup requires a Unix host")
    }
    pub(crate) fn token(&self) -> &str {
        ""
    }
    pub(crate) fn launch_id(&self) -> &str {
        ""
    }
    pub(crate) fn publish(&mut self, _: SocketAddr) -> Result<()> {
        bail!("managed desktop startup requires a Unix host")
    }
}

#[cfg(unix)]
mod unix {
    use super::*;
    use anyhow::Context;
    use rustix::fs::{AtFlags, Mode, OFlags};
    use serde::Serialize;
    use std::{
        ffi::OsString,
        fs::File,
        io::{Read, Write},
        os::unix::fs::MetadataExt,
        path::{Component, Path},
    };

    const DIR_FLAGS: OFlags = OFlags::RDONLY
        .union(OFlags::DIRECTORY)
        .union(OFlags::NOFOLLOW)
        .union(OFlags::CLOEXEC);
    const READ_FLAGS: OFlags = OFlags::RDONLY
        .union(OFlags::NOFOLLOW)
        .union(OFlags::CLOEXEC)
        .union(OFlags::NONBLOCK);
    const MAX_TOKEN_BYTES: u64 = 8192;

    pub(crate) struct Bootstrap {
        token: String,
        launch_id: String,
        directory: File,
        name: OsString,
        published: Option<(u64, u64)>,
    }

    #[derive(Serialize)]
    struct Receipt<'a> {
        schema: &'static str,
        launch_id: &'a str,
        endpoint: String,
        pid: u32,
        version: &'static str,
    }

    impl Bootstrap {
        pub(super) fn open(
            token_path: PathBuf,
            ready_path: PathBuf,
            launch_id: String,
        ) -> Result<Self> {
            ensure!(
                token_path != ready_path,
                "token and readiness files must be different"
            );
            let (token_parent, token_name) = private_parent(&token_path)?;
            let file = File::from(
                rustix::fs::openat(&token_parent, &token_name, READ_FLAGS, Mode::empty())
                    .context("open private token file")?,
            );
            let metadata = file.metadata()?;
            ensure!(
                metadata.is_file()
                    && metadata.uid() == rustix::process::geteuid().as_raw()
                    && metadata.mode() & 0o7777 == 0o600
                    && metadata.nlink() == 1,
                "token file must be a regular owner-only mode-0600 file with one link"
            );
            let mut bytes = Vec::new();
            file.take(MAX_TOKEN_BYTES + 1)
                .read_to_end(&mut bytes)
                .context("read private token file")?;
            ensure!(
                bytes.len() as u64 <= MAX_TOKEN_BYTES,
                "token file exceeds the size limit"
            );
            let text = String::from_utf8(bytes).context("token file is not UTF-8")?;
            let token = text.trim_end_matches(['\r', '\n']);
            ensure!(
                token.len() >= 16
                    && token
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"-._~+/=".contains(&b)),
                "token file must contain a bearer token of at least 16 ASCII characters"
            );
            let (directory, name) = private_parent(&ready_path)?;
            match rustix::fs::statat(&directory, &name, AtFlags::SYMLINK_NOFOLLOW) {
                Err(rustix::io::Errno::NOENT) => {}
                Ok(_) => bail!("readiness path already exists; use a new launch directory"),
                Err(error) => return Err(error).context("check new readiness path"),
            }
            Ok(Self {
                token: token.to_owned(),
                launch_id,
                directory,
                name,
                published: None,
            })
        }

        pub(crate) fn token(&self) -> &str {
            &self.token
        }

        pub(crate) fn launch_id(&self) -> &str {
            &self.launch_id
        }

        pub(crate) fn publish(&mut self, address: SocketAddr) -> Result<()> {
            ensure!(
                self.published.is_none(),
                "readiness has already been published"
            );
            ensure!(
                address.ip() == IpAddr::V4(Ipv4Addr::LOCALHOST) && address.port() != 0,
                "managed listener did not bind an assigned IPv4 loopback port"
            );
            let temporary = format!(".rupu-ready-{}.tmp", ulid::Ulid::new());
            let mut file = File::from(
                rustix::fs::openat(
                    &self.directory,
                    temporary.as_str(),
                    OFlags::WRONLY
                        | OFlags::CREATE
                        | OFlags::EXCL
                        | OFlags::NOFOLLOW
                        | OFlags::CLOEXEC,
                    Mode::RUSR | Mode::WUSR,
                )
                .context("create private readiness staging file")?,
            );
            let metadata = file.metadata()?;
            let identity = (metadata.dev(), metadata.ino());
            let result = (|| -> Result<()> {
                let receipt = Receipt {
                    schema: "rupu.cp.bootstrap.v1",
                    launch_id: &self.launch_id,
                    endpoint: format!("http://{address}"),
                    pid: std::process::id(),
                    version: env!("CARGO_PKG_VERSION"),
                };
                serde_json::to_writer(&mut file, &receipt)?;
                file.write_all(b"\n")?;
                file.sync_all()?;
                // linkat creates the completed file atomically and refuses an
                // existing destination, including a symlink. It never replaces.
                rustix::fs::linkat(
                    &self.directory,
                    temporary.as_str(),
                    &self.directory,
                    &self.name,
                    AtFlags::empty(),
                )
                .context("publish new readiness receipt without replacement")?;
                self.published = Some(identity);
                self.directory.sync_all()?;
                Ok(())
            })();
            unlink_owned(&self.directory, temporary.as_ref(), identity);
            if result.is_err() {
                self.remove_receipt();
            }
            result
        }

        fn remove_receipt(&mut self) {
            if let Some(identity) = self.published.take() {
                unlink_owned(&self.directory, &self.name, identity);
            }
        }
    }

    impl Drop for Bootstrap {
        fn drop(&mut self) {
            self.remove_receipt();
        }
    }

    // rustix's stat fields vary by platform; MetadataExt normalizes to u64.
    #[allow(clippy::unnecessary_cast)]
    fn unlink_owned(directory: &File, name: &std::ffi::OsStr, identity: (u64, u64)) {
        if rustix::fs::statat(directory, name, AtFlags::SYMLINK_NOFOLLOW)
            .is_ok_and(|value| (value.st_dev as u64, value.st_ino as u64) == identity)
        {
            let _ = rustix::fs::unlinkat(directory, name, AtFlags::empty());
        }
    }

    /// Walk every absolute component without following symlinks; retain the
    /// parent descriptor so later publication cannot be redirected by rename.
    fn private_parent(path: &Path) -> Result<(File, OsString)> {
        ensure!(path.is_absolute(), "managed startup paths must be absolute");
        let components: Vec<_> = path.components().collect();
        ensure!(
            components.len() >= 2
                && components[1..]
                    .iter()
                    .all(|c| matches!(c, Component::Normal(_))),
            "managed startup paths must contain only ordinary absolute components"
        );
        let mut directory = File::from(rustix::fs::open("/", DIR_FLAGS, Mode::empty())?);
        for component in &components[1..components.len() - 1] {
            directory = File::from(
                rustix::fs::openat(&directory, component.as_os_str(), DIR_FLAGS, Mode::empty())
                    .context("open managed startup directory without symlinks")?,
            );
        }
        let metadata = directory.metadata()?;
        ensure!(
            metadata.uid() == rustix::process::geteuid().as_raw()
                && metadata.mode() & 0o7777 == 0o700,
            "managed startup parent directory must be owned by this user with mode 0700"
        );
        Ok((
            directory,
            components
                .last()
                .expect("checked length")
                .as_os_str()
                .to_owned(),
        ))
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::os::unix::fs::{symlink, PermissionsExt};

        const ID: &str = "352b0b5b-8851-4c1c-9c91-5c347bdc2f18";
        const TOKEN: &str = "private-bootstrap-token-for-unit-test";
        fn setup() -> (tempfile::TempDir, PathBuf, PathBuf) {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().canonicalize().unwrap();
            std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
            let token = root.join("token");
            let ready = root.join("ready.json");
            std::fs::write(&token, TOKEN).unwrap();
            std::fs::set_permissions(&token, std::fs::Permissions::from_mode(0o600)).unwrap();
            (temp, token, ready)
        }
        #[test]
        fn receipt_is_private_bound_token_free_and_removed_only_when_owned() {
            let (_temp, token, ready) = setup();
            let mut value = Bootstrap::open(token.clone(), ready.clone(), ID.into()).unwrap();
            value.publish("127.0.0.1:12345".parse().unwrap()).unwrap();
            let bytes = std::fs::read(&ready).unwrap();
            let receipt: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(receipt["schema"], "rupu.cp.bootstrap.v1");
            assert_eq!(receipt["launch_id"], ID);
            assert_eq!(receipt["endpoint"], "http://127.0.0.1:12345");
            assert_eq!(receipt["pid"], std::process::id());
            assert!(!String::from_utf8(bytes).unwrap().contains(TOKEN));
            assert_eq!(std::fs::metadata(&ready).unwrap().mode() & 0o777, 0o600);
            drop(value);
            assert!(!ready.exists());
            assert!(token.exists());
        }
        #[test]
        fn replacement_receipt_is_never_deleted_by_cleanup() {
            let (_temp, token, ready) = setup();
            let mut value = Bootstrap::open(token, ready.clone(), ID.into()).unwrap();
            value.publish("127.0.0.1:12345".parse().unwrap()).unwrap();
            // Keep the old inode alive so the replacement cannot reuse it.
            std::fs::rename(&ready, ready.with_extension("previous")).unwrap();
            std::fs::write(&ready, "replacement").unwrap();
            drop(value);
            assert_eq!(std::fs::read_to_string(ready).unwrap(), "replacement");
        }
        #[test]
        fn refusal_never_overwrites_existing_or_symlink_receipt() {
            let (_temp, token, ready) = setup();
            std::fs::write(&ready, "existing").unwrap();
            assert!(Bootstrap::open(token.clone(), ready.clone(), ID.into()).is_err());
            std::fs::remove_file(&ready).unwrap();
            symlink(&token, &ready).unwrap();
            assert!(Bootstrap::open(token.clone(), ready, ID.into()).is_err());
            assert_eq!(std::fs::read_to_string(token).unwrap(), TOKEN);
        }
        #[test]
        fn receipt_created_after_preflight_is_not_replaced() {
            let (_temp, token, ready) = setup();
            let mut value = Bootstrap::open(token, ready.clone(), ID.into()).unwrap();
            std::fs::write(&ready, "another launch").unwrap();
            assert!(value.publish("127.0.0.1:12345".parse().unwrap()).is_err());
            assert_eq!(std::fs::read_to_string(ready).unwrap(), "another launch");
        }
        #[test]
        fn token_requires_private_regular_single_link_file_and_directory() {
            let (_temp, token, ready) = setup();
            std::fs::set_permissions(&token, std::fs::Permissions::from_mode(0o644)).unwrap();
            assert!(Bootstrap::open(token.clone(), ready.clone(), ID.into()).is_err());
            std::fs::set_permissions(&token, std::fs::Permissions::from_mode(0o600)).unwrap();
            let alias = token.with_extension("alias");
            std::fs::hard_link(&token, &alias).unwrap();
            assert!(Bootstrap::open(token.clone(), ready.clone(), ID.into()).is_err());
            std::fs::remove_file(alias).unwrap();
            std::fs::set_permissions(
                token.parent().unwrap(),
                std::fs::Permissions::from_mode(0o755),
            )
            .unwrap();
            assert!(Bootstrap::open(token, ready, ID.into()).is_err());
        }
        #[test]
        fn token_and_directory_symlinks_are_refused() {
            let (_temp, token, ready) = setup();
            let alias = token.with_extension("alias");
            symlink(&token, &alias).unwrap();
            assert!(Bootstrap::open(alias, ready.clone(), ID.into()).is_err());
            let parent_alias = token.parent().unwrap().join("parent-link");
            symlink(token.parent().unwrap(), &parent_alias).unwrap();
            assert!(Bootstrap::open(parent_alias.join("token"), ready, ID.into()).is_err());
        }
        #[test]
        fn malformed_oversize_or_short_token_is_refused_without_echoing_secret() {
            for contents in [
                "short".to_owned(),
                "a".repeat(8193),
                "has a space in secret".into(),
            ] {
                let (_temp, token, ready) = setup();
                std::fs::write(&token, &contents).unwrap();
                let error = Bootstrap::open(token, ready, ID.into())
                    .err()
                    .unwrap()
                    .to_string();
                assert!(!error.contains(&contents));
            }
        }
    }
}

#[cfg(test)]
mod contract_tests {
    use super::*;
    #[test]
    fn normal_startup_is_unchanged_and_partial_flags_are_refused() {
        assert!(prepare("127.0.0.1:0".parse().unwrap(), None, None, None)
            .unwrap()
            .is_none());
        assert!(prepare(
            "127.0.0.1:0".parse().unwrap(),
            Some("/unused".into()),
            None,
            None
        )
        .is_err());
    }
    #[test]
    fn uuid_and_exact_loopback_are_required_before_file_reads() {
        assert!(valid_launch_id("352b0b5b-8851-4c1c-9c91-5c347bdc2f18"));
        for bad in ["", "../escape", "352b0b5b-8851-4c1c-9c91-5c347bdc2f1z"] {
            assert!(!valid_launch_id(bad));
        }
        assert!(prepare(
            "0.0.0.0:0".parse().unwrap(),
            Some("/unused".into()),
            Some("/unused2".into()),
            Some("352b0b5b-8851-4c1c-9c91-5c347bdc2f18".into())
        )
        .is_err());
    }
}

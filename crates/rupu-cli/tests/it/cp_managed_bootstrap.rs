//! The native startup handoff against an actual CLI child. Each child owns a
//! disposable home, has no inherited credentials, and binds only an OS-selected
//! loopback port. These tests never discover, stop, or modify another CP.
#![cfg(unix)]

use serde_json::Value;
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

const ID: &str = "352b0b5b-8851-4c1c-9c91-5c347bdc2f18";
const TOKEN: &str = "private-integration-bootstrap-bearer-token";

struct Fixture {
    _temporary: tempfile::TempDir,
    root: PathBuf,
    token: PathBuf,
    receipt: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let home = root.join(".rupu");
        std::fs::create_dir(&home).unwrap();
        std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::write(
            home.join("config.toml"),
            "[cp]\nautoflow_reconcile_enabled = false\ncron_tick_enabled = false\ngate_sweep_enabled = false\n\
             [netflow]\nasn_auto_refresh = false\n\
             [agentiflow]\nreaper_enabled = false\n",
        )
        .unwrap();
        let token = root.join("token");
        let receipt = root.join("ready.json");
        std::fs::write(&token, TOKEN).unwrap();
        std::fs::set_permissions(&token, std::fs::Permissions::from_mode(0o600)).unwrap();
        Self {
            _temporary: temporary,
            root,
            token,
            receipt,
        }
    }

    fn command(&self, name: &str) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_rupu"));
        command
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("RUPU_HOME", self.root.join(".rupu"))
            .env("RUPU_AUTH_FILE", self.root.join(".rupu/auth.json"))
            .env("RUPU_NO_UPDATE_CHECK", "1")
            .current_dir(&self.root)
            .args(["cp", "serve", "--bind", "127.0.0.1:0", "--no-open"])
            .stdin(Stdio::null())
            .stdout(std::fs::File::create(self.root.join(format!("{name}.stdout"))).unwrap())
            .stderr(std::fs::File::create(self.root.join(format!("{name}.stderr"))).unwrap());
        command
    }

    fn managed(&self, name: &str) -> Command {
        let mut command = self.command(name);
        command
            .arg("--token-file")
            .arg(&self.token)
            .arg("--ready-file")
            .arg(&self.receipt)
            .args(["--launch-id", ID]);
        command
    }

    fn assert_output_is_private(&self, name: &str) {
        for stream in ["stdout", "stderr"] {
            let output =
                std::fs::read_to_string(self.root.join(format!("{name}.{stream}"))).unwrap();
            assert!(!output.contains(TOKEN), "secret leaked in {stream}");
            assert!(
                !output.contains("?token="),
                "access-bearing URL leaked in {stream}"
            );
            assert!(
                !output.contains("Authorization: Bearer"),
                "credential example leaked in {stream}"
            );
        }
    }
}

struct OwnedChild(Child);
impl OwnedChild {
    fn wait(&mut self) -> ExitStatus {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(status) = self.0.try_wait().unwrap() {
                return status;
            }
            assert!(Instant::now() < deadline, "owned CLI child did not exit");
            std::thread::sleep(Duration::from_millis(25));
        }
    }
}
impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn get(endpoint: &str, path: &str, token: Option<&str>) -> std::io::Result<(u16, String, String)> {
    let address = endpoint.strip_prefix("http://").unwrap();
    let address: std::net::SocketAddr = address.parse().unwrap();
    assert_eq!(address.ip(), std::net::Ipv4Addr::LOCALHOST);
    assert_ne!(address.port(), 0);
    let mut socket = std::net::TcpStream::connect_timeout(&address, Duration::from_secs(2))?;
    socket.set_read_timeout(Some(Duration::from_secs(2)))?;
    let authorization = token
        .map(|value| format!("Authorization: Bearer {value}\r\n"))
        .unwrap_or_default();
    socket.write_all(
        format!(
            "GET {path} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n{authorization}\r\n"
        )
        .as_bytes(),
    )?;
    let mut response = String::new();
    socket.read_to_string(&mut response)?;
    let (head, body) = response.split_once("\r\n\r\n").unwrap();
    let status = head.split_whitespace().nth(1).unwrap().parse().unwrap();
    Ok((status, head.to_lowercase(), body.to_string()))
}

fn json_body(body: &str) -> Value {
    let first = body.find('{').expect("JSON response");
    let last = body.rfind('}').expect("JSON response");
    serde_json::from_str(&body[first..=last]).unwrap()
}

#[test]
fn managed_port_zero_receipt_matches_authenticated_owned_process() {
    let fixture = Fixture::new();
    let mut child = OwnedChild(fixture.managed("server").spawn().unwrap());
    let deadline = Instant::now() + Duration::from_secs(30);
    let receipt: Value = loop {
        if let Ok(bytes) = std::fs::read(&fixture.receipt) {
            break serde_json::from_slice(&bytes).expect("atomic complete JSON receipt");
        }
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "owned child exited before receipt"
        );
        assert!(Instant::now() < deadline, "no startup receipt");
        std::thread::sleep(Duration::from_millis(25));
    };
    assert_eq!(receipt["schema"], "rupu.cp.bootstrap.v1");
    assert_eq!(receipt["launch_id"], ID);
    assert_eq!(receipt["pid"], child.0.id());
    assert_eq!(receipt["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(
        std::fs::metadata(&fixture.receipt).unwrap().mode() & 0o777,
        0o600
    );
    assert!(!receipt.to_string().contains(TOKEN));
    let endpoint = receipt["endpoint"].as_str().unwrap();
    let headers = loop {
        if let Ok((200, headers, body)) = get(endpoint, "/healthz", None) {
            assert_eq!(body.trim(), "ok");
            break headers;
        }
        assert!(child.0.try_wait().unwrap().is_none());
        assert!(
            Instant::now() < deadline,
            "bound receipt did not become HTTP-ready"
        );
        std::thread::sleep(Duration::from_millis(25));
    };
    assert!(headers.contains(&format!("x-rupu-pid: {}", child.0.id())));
    assert!(headers.contains(&format!("x-rupu-launch-id: {ID}")));
    assert_eq!(get(endpoint, "/api/host/info", None).unwrap().0, 401);
    assert_eq!(
        get(endpoint, "/api/host/info", Some("incorrect-token"))
            .unwrap()
            .0,
        401
    );
    let (status, _, body) = get(endpoint, "/api/host/info", Some(TOKEN)).unwrap();
    assert_eq!(status, 200);
    let info = json_body(&body);
    assert_eq!(info["version"], receipt["version"]);
    assert!(info["features"].is_array());
    let (status, _, body) = get(endpoint, "/api/config", Some(TOKEN)).unwrap();
    assert_eq!(status, 200);
    assert_eq!(
        json_body(&body)["status"]["bind"],
        endpoint.strip_prefix("http://").unwrap()
    );
    // A second launch cannot replace the first launch's receipt or token.
    let original = std::fs::read(&fixture.receipt).unwrap();
    let mut duplicate = OwnedChild(fixture.managed("duplicate").spawn().unwrap());
    assert!(!duplicate.wait().success());
    assert_eq!(std::fs::read(&fixture.receipt).unwrap(), original);
    assert_eq!(std::fs::read_to_string(&fixture.token).unwrap(), TOKEN);
    assert_eq!(get(endpoint, "/healthz", None).unwrap().0, 200);
    fixture.assert_output_is_private("duplicate");
    drop(child); // signal only the Child this test created, never a receipt PID.
    fixture.assert_output_is_private("server");
}

#[test]
fn managed_private_paths_and_complete_flags_are_enforced_before_start() {
    let fixture = Fixture::new();
    std::fs::set_permissions(&fixture.token, std::fs::Permissions::from_mode(0o644)).unwrap();
    let mut child = OwnedChild(fixture.managed("permissions").spawn().unwrap());
    assert!(!child.wait().success());
    assert!(!fixture.receipt.exists());
    fixture.assert_output_is_private("permissions");
    std::fs::set_permissions(&fixture.token, std::fs::Permissions::from_mode(0o600)).unwrap();
    let mut command = fixture.command("partial");
    command.arg("--token-file").arg(&fixture.token);
    let mut child = OwnedChild(command.spawn().unwrap());
    assert!(!child.wait().success());
    assert!(!fixture.receipt.exists());
    fixture.assert_output_is_private("partial");
    std::os::unix::fs::symlink(&fixture.token, &fixture.receipt).unwrap();
    let mut child = OwnedChild(fixture.managed("symlink").spawn().unwrap());
    assert!(!child.wait().success());
    assert_eq!(std::fs::read_to_string(&fixture.token).unwrap(), TOKEN);
    fixture.assert_output_is_private("symlink");
}

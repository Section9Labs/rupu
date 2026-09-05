//! `rupu cp serve` must exit non-zero, promptly, when its bind address is
//! already taken — even when its background loops would otherwise be busy.
//!
//! Regression for 2026-09-04: the listener was bound only AFTER the
//! resume worker / cron / autoflow / gate-sweep / fleet-inventory loops
//! were spawned, and the bind-failure path then waited for every loop to
//! drain. A loop mid-tick (the fleet-inventory SCM refresh runs at spawn
//! and makes one GitHub call per repo) does not observe shutdown until
//! its tick finishes, so a second `cp serve` sat for minutes with no
//! listener, an open HTTPS connection to GitHub, and no error printed.
//!
//! Mutates process-global env (`RUPU_HOME`, `RUPU_AUTH_FILE`); this test
//! is alone in its binary on purpose.

use std::process::ExitCode;
use std::time::{Duration, Instant};

#[tokio::test(flavor = "multi_thread")]
async fn cp_serve_exits_promptly_when_bind_address_is_taken() {
    // Same one-time provider install `main.rs` does; the inventory loops
    // build TLS-capable HTTP clients and rustls needs a provider chosen.
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let tmp = assert_fs::TempDir::new().unwrap();

    // The occupant: a plain socket holding the port `cp serve` wants.
    let occupant = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let bind = occupant.local_addr().unwrap();

    // A blackhole "GitHub": accepts connections and never answers. Any
    // background loop that reaches for SCM is then stuck for the configured
    // timeout — which is exactly the state the bind failure used to wait on.
    let blackhole = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let github = blackhole.local_addr().unwrap();
    tokio::spawn(async move {
        let mut held = Vec::new();
        loop {
            if let Ok((sock, _)) = blackhole.accept().await {
                held.push(sock);
            }
        }
    });

    std::fs::write(
        tmp.path().join("config.toml"),
        format!("[scm.github]\nbase_url = \"http://{github}\"\ntimeout_ms = 60000\n"),
    )
    .unwrap();
    // Legacy bare-key credential entry: enough for the GitHub connector to
    // register, which is what makes the inventory refresh go to the network.
    let auth = tmp.path().join("auth.json");
    std::fs::write(&auth, r#"{"github":"ghp_test"}"#).unwrap();
    std::env::set_var("RUPU_HOME", tmp.path());
    std::env::set_var("RUPU_AUTH_FILE", &auth);

    let args: Vec<String> = [
        "rupu",
        "cp",
        "serve",
        "--bind",
        &bind.to_string(),
        "--no-open",
    ]
    .into_iter()
    .map(String::from)
    .collect();

    let started = Instant::now();
    let exit = tokio::time::timeout(Duration::from_secs(20), rupu_cli::run(args))
        .await
        .expect("cp serve must not linger after a bind failure");

    assert_eq!(
        format!("{exit:?}"),
        format!("{:?}", ExitCode::FAILURE),
        "a taken bind address must be a non-zero exit"
    );
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "bind failure took {:?}; it must not wait on background loops",
        started.elapsed()
    );
}

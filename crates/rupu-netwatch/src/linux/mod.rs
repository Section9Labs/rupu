//! Linux capture backend. The modules here are pure (no syscalls, no
//! `cfg`): they decode and build netlink bytes and make decisions over plain
//! inputs, so they compile and are tested on every platform. The socket,
//! cgroup filesystem and watcher-thread code lives in
//! `cfg(target_os = "linux")` modules and items.

pub mod cgroup;
#[cfg(target_os = "linux")]
pub mod netlink;
pub mod parse;
pub mod req;
#[cfg(target_os = "linux")]
mod watch;

#[cfg(target_os = "linux")]
pub use capture::{LinuxCapture, LinuxCaptureCall};

#[cfg(target_os = "linux")]
mod capture {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;
    use std::thread::JoinHandle;
    use std::time::Duration;

    use chrono::Utc;
    use rupu_netflow::{
        CallAttribution, CaptureCall, CaptureState, CaptureStateLine, NoopCapture,
        SubprocessCapture,
    };

    use super::cgroup::{setup_root, CaptureRoot};
    use super::netlink::DiagSocket;
    use super::watch::{self, lock, CgEntry, Shared, BACKEND, DESTROY_RCVBUF};
    use crate::tracker::CallInfo;
    use crate::types::CallId;

    /// The Linux backend: one cgroup per bash call, attributed to sockets by
    /// a `sock_diag` watcher thread feeding the pure tracker.
    pub struct LinuxCapture {
        shared: Arc<Shared>,
        root: Arc<CaptureRoot>,
        seq: AtomicU64,
        thread: Option<JoinHandle<()>>,
    }

    impl LinuxCapture {
        /// Obtain a capture cgroup root, open the sockets and start the
        /// watcher thread. `linger` is how long a finished call stays
        /// attributable; `poll` is the watcher's cycle (its socket receive
        /// timeout). `Err` carries the reason capture is unavailable, for
        /// the caller to fall back to an inert backend.
        ///
        /// This BLOCKS (see [`setup_root`]): call it from a plain thread or
        /// `spawn_blocking`.
        pub fn start(linger: chrono::Duration, poll: Duration) -> Result<Self, String> {
            let root = setup_root()?;
            match Self::start_with_root(root.clone(), linger, poll) {
                Ok(c) => Ok(c),
                Err(e) => {
                    root.shutdown();
                    Err(e)
                }
            }
        }

        fn start_with_root(
            root: CaptureRoot,
            linger: chrono::Duration,
            poll: Duration,
        ) -> Result<Self, String> {
            let dump =
                DiagSocket::open().map_err(|e| format!("cannot open sock_diag socket: {e}"))?;
            let destroy =
                DiagSocket::open().map_err(|e| format!("cannot open sock_diag socket: {e}"))?;
            destroy
                .bind_destroy_groups()
                .map_err(|e| format!("cannot join the sock_diag destroy groups: {e}"))?;
            destroy.set_rcvbuf(DESTROY_RCVBUF);
            dump.set_rcvbuf(1 << 20);
            for s in [&dump, &destroy] {
                s.set_recv_timeout(Some(poll))
                    .map_err(|e| format!("cannot set the sock_diag receive timeout: {e}"))?;
            }
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| format!("cannot build the netwatch runtime: {e}"))?;

            let shared = Arc::new(Shared::new(linger));
            let thread = watch::spawn(shared.clone(), dump, destroy, rt, poll)
                .map_err(|e| format!("cannot spawn the netwatch thread: {e}"))?;
            Ok(Self {
                shared,
                root: Arc::new(root),
                seq: AtomicU64::new(1),
                thread: Some(thread),
            })
        }
    }

    impl SubprocessCapture for LinuxCapture {
        fn begin(&self, call: CallAttribution) -> Box<dyn CaptureCall> {
            let seq: CallId = self.seq.fetch_add(1, Ordering::SeqCst);
            let cg = match self.root.create_call(seq) {
                Ok(cg) => cg,
                Err(e) => {
                    // Best effort: the bash call must still run.
                    tracing::warn!(error = %e, "netwatch: cannot create a call cgroup");
                    return NoopCapture.begin(call);
                }
            };
            let now = Utc::now();
            let prefix = cg.shell_prefix();
            let entry = CgEntry {
                cg: cg.clone(),
                run_id: call.run_id.clone(),
                tool_call_id: call.tool_call_id.clone(),
                sink: call.sink.clone(),
                finished: None,
                loss_noted: false,
            };
            // Announce capture once per run, so an empty ledger reads as
            // "observed, nothing connected" rather than "unknown".
            if lock(&self.shared.announced).insert(call.run_id.clone()) {
                self.shared.queue_state(
                    call.sink.clone(),
                    CaptureStateLine {
                        state: CaptureState::Active {
                            backend: BACKEND.to_string(),
                        },
                        tool_call_id: Some(call.tool_call_id.clone()),
                        note: None,
                    },
                );
            }
            lock(&self.shared.tracker).register_call(
                seq,
                CallInfo {
                    owner: cg.id,
                    attribution: call,
                },
                now,
            );
            lock(&self.shared.cgroups).insert(seq, entry);
            Box::new(LinuxCaptureCall {
                seq,
                prefix,
                shared: self.shared.clone(),
            })
        }

        fn run_finished(&self, run_id: &str) {
            let emissions = lock(&self.shared.tracker).finish_run(run_id, Utc::now());
            self.shared.release_run(run_id, Utc::now());
            self.shared.flush_blocking(emissions);
        }
    }

    impl Drop for LinuxCapture {
        fn drop(&mut self) {
            self.shared.running.store(false, Ordering::Release);
            if let Some(t) = self.thread.take() {
                let _ = t.join();
            }
            self.root.shutdown();
        }
    }

    /// One tracked bash call: its cgroup is entered by the shell prefix.
    pub struct LinuxCaptureCall {
        seq: CallId,
        prefix: String,
        shared: Arc<Shared>,
    }

    impl CaptureCall for LinuxCaptureCall {
        fn shell_prefix(&self) -> Option<String> {
            Some(self.prefix.clone())
        }

        // Attribution is by cgroup, which the prefix already joined: the
        // pid is not needed.
        fn spawned(&mut self, _pid: u32) {}

        fn finished(self: Box<Self>) {
            let now = Utc::now();
            lock(&self.shared.tracker).finish_call(self.seq, now);
            // The watcher's tick drops the call after the linger and removes
            // its cgroup then.
            if let Some(e) = lock(&self.shared.cgroups).get_mut(&self.seq) {
                e.finished.get_or_insert(now);
            }
        }
    }

    #[cfg(test)]
    mod live_tests {
        use super::*;
        use rupu_netflow::{Fidelity, MemorySink, Origin, Outcome};
        use std::process::Command;
        use std::thread::sleep;

        #[test]
        #[ignore = "needs a delegated cgroup v2 environment, curl and network access"]
        fn live_capture_attributes_subprocess_sockets() {
            let cap = LinuxCapture::start(chrono::Duration::seconds(3), Duration::from_millis(50))
                .expect("start capture");
            let sink = Arc::new(MemorySink::default());
            let call = cap.begin(CallAttribution {
                run_id: "run-live".into(),
                step_id: Some("step-live".into()),
                agent: Some("agent-live".into()),
                codename: None,
                tool_call_id: "call-live".into(),
                sink: sink.clone(),
            });
            let prefix = call.shell_prefix().expect("a shell prefix");
            // `--limit-rate` holds the first connection ESTABLISHED for
            // ~3s so a poll is certain to catch it; the second targets a
            // blackhole and never establishes.
            let script = format!(
                "{prefix}; curl -s -o /dev/null --limit-rate 100 --max-time 3 http://example.com; \
                 curl -s -o /dev/null --connect-timeout 1 http://10.255.255.1:9 || true"
            );
            let status = Command::new("/bin/sh")
                .arg("-c")
                .arg(script)
                .status()
                .expect("run sh");
            assert!(status.code().is_some());
            sleep(Duration::from_secs(1));
            call.finished();
            // Past the 3s linger plus a poll or two.
            sleep(Duration::from_secs(4));
            cap.run_finished("run-live");

            let records = sink.records();
            let socket_flows: Vec<_> = records
                .iter()
                .filter(|r| r.fidelity == Fidelity::Socket)
                .collect();
            for r in &socket_flows {
                assert!(matches!(r.ctx.origin, Origin::Subprocess(_)), "{:?}", r.ctx);
                assert_eq!(r.ctx.tool_call_id.as_deref(), Some("call-live"));
                assert_eq!(r.ctx.run_id.as_deref(), Some("run-live"));
            }
            let http = socket_flows
                .iter()
                .find(|r| r.port == 80)
                .unwrap_or_else(|| panic!("no port-80 socket flow in {records:#?}"));
            let ip = http.peer_ip.expect("peer ip");
            assert!(!ip.is_loopback() && !ip.is_unspecified(), "{ip}");

            // Completion arrives as a socket completion folded by id.
            let completed = sink.socket_completions().iter().any(|c| c.id == http.id);
            assert!(completed, "no completion for the http flow");

            // The blackhole may be refused outright on some networks, so
            // this is reported rather than asserted.
            match socket_flows.iter().find(|r| r.port == 9) {
                Some(r) => assert_eq!(r.outcome, Outcome::TransportError),
                None => eprintln!("note: no never-established flow for 10.255.255.1:9"),
            }
        }
    }
}

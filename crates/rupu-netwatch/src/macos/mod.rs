//! macOS capture backend. `parse` is pure (no syscalls, no `cfg`): it decodes
//! `ntstat` kernel-control messages from bytes and is tested on every
//! platform. The socket, process-tree and watcher code lives in
//! `cfg(target_os = "macos")` modules.
//!
//! Attribution is by ancestry, not cgroup: the bash tool reports the shell's
//! pid through `spawned()`, and a socket belongs to that call when its
//! owning process descends from the shell.

pub mod parse;

#[cfg(target_os = "macos")]
pub mod ntstat;
#[cfg(target_os = "macos")]
pub mod proctree;
#[cfg(target_os = "macos")]
mod watch;

#[cfg(target_os = "macos")]
pub use capture::{MacosCapture, MacosCaptureCall};

#[cfg(target_os = "macos")]
mod capture {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;
    use std::thread::JoinHandle;

    use chrono::Utc;
    use rupu_netflow::{
        CallAttribution, CaptureCall, CaptureState, CaptureStateLine, NoopCapture,
        SubprocessCapture,
    };

    use super::ntstat::NstatSocket;
    use super::watch::{self, lock, CallEntry, Shared, BACKEND, TICK};
    use crate::tracker::CallInfo;
    use crate::types::CallId;

    /// Call numbering, process-global so several instances (tests) never
    /// collide.
    static SEQ: AtomicU64 = AtomicU64::new(1);

    /// The macOS backend: a watcher thread reads the kernel's
    /// `com.apple.network.statistics` feed and attributes sockets to bash
    /// calls by process ancestry. Meant to be a process singleton.
    pub struct MacosCapture {
        shared: Arc<Shared>,
        thread: Option<JoinHandle<()>>,
    }

    impl MacosCapture {
        /// Open the `ntstat` socket, subscribe to every TCP/UDP source and
        /// start the watcher thread. `linger` is how long a finished call
        /// stays attributable. `Err` carries the reason capture is
        /// unavailable, for the caller to fall back to an inert backend.
        pub fn start(linger: chrono::Duration) -> Result<Self, String> {
            let sock = NstatSocket::open()
                .map_err(|e| format!("cannot open the network-statistics control socket: {e}"))?;
            sock.set_recv_timeout(Some(TICK))
                .map_err(|e| format!("cannot set the ntstat receive timeout: {e}"))?;
            sock.subscribe_all()
                .map_err(|e| format!("cannot subscribe to network statistics: {e}"))?;
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| format!("cannot build the netwatch runtime: {e}"))?;
            let shared = Arc::new(Shared::new(linger));
            let thread = watch::spawn(shared.clone(), sock, rt)
                .map_err(|e| format!("cannot spawn the netwatch thread: {e}"))?;
            Ok(Self {
                shared,
                thread: Some(thread),
            })
        }
    }

    impl SubprocessCapture for MacosCapture {
        fn begin(&self, call: CallAttribution) -> Box<dyn CaptureCall> {
            if self.thread.as_ref().is_some_and(|t| t.is_finished()) {
                // The watcher died: nothing would tick or reap new calls.
                // Degrade visibly (once per run) and stay inert.
                tracing::error!("netwatch: the watcher thread is not running");
                if lock(&self.shared.announced).insert(format!("dead:{}", call.run_id)) {
                    self.shared.queue_state(
                        call.sink.clone(),
                        CaptureStateLine {
                            state: CaptureState::Unavailable {
                                reason: "the subprocess capture watcher stopped".to_string(),
                            },
                            tool_call_id: Some(call.tool_call_id.clone()),
                            note: None,
                        },
                    );
                    self.shared.flush_blocking();
                }
                return NoopCapture.begin(call);
            }
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
            // The call registers with the tracker in `spawned()`: its
            // owner is the shell's pid, which is not known yet.
            Box::new(MacosCaptureCall {
                seq: SEQ.fetch_add(1, Ordering::SeqCst),
                attribution: Some(call),
                registered: false,
                shared: self.shared.clone(),
            })
        }

        fn run_finished(&self, run_id: &str) {
            self.shared.track(|t| t.finish_run(run_id, Utc::now()));
            self.shared.release_run(run_id, Utc::now());
            self.shared.flush_blocking();
        }
    }

    impl Drop for MacosCapture {
        fn drop(&mut self) {
            self.shared.running.store(false, Ordering::Release);
            if let Some(t) = self.thread.take() {
                let _ = t.join();
            }
        }
    }

    /// One tracked bash call. Needs no shell prefix: attribution is by
    /// ancestry from the pid `spawned()` reports.
    pub struct MacosCaptureCall {
        seq: CallId,
        attribution: Option<CallAttribution>,
        registered: bool,
        shared: Arc<Shared>,
    }

    impl CaptureCall for MacosCaptureCall {
        fn shell_prefix(&self) -> Option<String> {
            None
        }

        fn spawned(&mut self, pid: u32) {
            let Some(attribution) = self.attribution.take() else {
                return;
            };
            let now = Utc::now();
            // Entry first, so the watcher never sees the call in the
            // tracker without its shell in the registered set.
            lock(&self.shared.calls).insert(
                self.seq,
                CallEntry {
                    shell: pid,
                    run_id: attribution.run_id.clone(),
                    tool_call_id: attribution.tool_call_id.clone(),
                    sink: attribution.sink.clone(),
                    finished: None,
                    loss_noted: false,
                },
            );
            lock(&self.shared.tracker).register_call(
                self.seq,
                CallInfo {
                    owner: pid as u64,
                    attribution,
                },
                now,
            );
            self.registered = true;
        }

        fn finished(self: Box<Self>) {
            if !self.registered {
                return;
            }
            let now = Utc::now();
            lock(&self.shared.tracker).finish_call(self.seq, now);
            // The watcher's tick drops the call after the linger and
            // unregisters the shell then.
            if let Some(e) = lock(&self.shared.calls).get_mut(&self.seq) {
                e.finished.get_or_insert(now);
            }
        }
    }

    #[cfg(test)]
    mod live_tests {
        use super::*;
        use rupu_netflow::{Fidelity, MemorySink, Origin};
        use std::process::Command;
        use std::thread::sleep;
        use std::time::Duration;

        #[test]
        #[serial_test::serial]
        #[ignore = "needs the live macOS network-statistics control, curl and network access"]
        fn live_capture_attributes_subprocess_sockets() {
            let cap = MacosCapture::start(chrono::Duration::seconds(3)).expect("start capture");
            let sink = Arc::new(MemorySink::default());
            let mut call = cap.begin(CallAttribution {
                run_id: "run-live".into(),
                step_id: Some("step-live".into()),
                agent: Some("agent-live".into()),
                codename: None,
                tool_call_id: "call-live".into(),
                sink: sink.clone(),
            });
            assert!(call.shell_prefix().is_none());
            // `--limit-rate` holds the connection ESTABLISHED for seconds so
            // a poll is certain to catch it.
            let mut child = Command::new("/bin/sh")
                .arg("-c")
                .arg("curl -s -o /dev/null --limit-rate 100 --max-time 6 http://example.com")
                .spawn()
                .expect("spawn sh");
            call.spawned(child.id());
            sleep(Duration::from_secs(2));
            let _ = child.wait();
            call.finished();
            // Past the 3s linger plus a tick or two.
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
            let proc = http.process.as_ref().expect("process");
            assert!(proc.name.contains("curl"), "{proc:?}");
            eprintln!("captured: {http:#?}");
            let completions = sink.socket_completions();
            let done = completions
                .iter()
                .find(|c| c.id == http.id)
                .unwrap_or_else(|| panic!("no completion for the http flow: {completions:#?}"));
            eprintln!("completion: {done:#?}");
        }
    }
}

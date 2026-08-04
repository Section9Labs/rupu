//! DNS resolver that records every answer.
//!
//! Delegates to the system resolver via `tokio::net::lookup_host` — the
//! same `getaddrinfo` path reqwest's default `GaiResolver` uses — and
//! keeps the most recent answer per host so the middleware can attach
//! `resolved_ips`.
//!
//! Caveat, deliberately not papered over: under concurrent requests to
//! the SAME host the map holds the latest answer, so `resolved_ips` is
//! "the most recent resolution for this host", not "the resolution this
//! request used". `peer_ip` is always exact — prefer it when the two
//! disagree.

use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
pub struct RecordingResolver {
    seen: Arc<Mutex<HashMap<String, Vec<IpAddr>>>>,
}

impl RecordingResolver {
    pub fn answers_for(&self, host: &str) -> Vec<IpAddr> {
        self.seen
            .lock()
            .ok()
            .and_then(|m| m.get(host).cloned())
            .unwrap_or_default()
    }
}

impl Resolve for RecordingResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let seen = self.seen.clone();
        let host = name.as_str().to_string();
        Box::pin(async move {
            let addrs: Vec<std::net::SocketAddr> =
                tokio::net::lookup_host((host.as_str(), 0)).await?.collect();
            if let Ok(mut m) = seen.lock() {
                m.insert(host, addrs.iter().map(|s| s.ip()).collect());
            }
            let iter: Addrs = Box::new(addrs.into_iter());
            Ok(iter)
        })
    }
}

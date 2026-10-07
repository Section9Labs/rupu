//! Process-group probes for the tests that drive detached processes
//! (agentiflow coordinators and their units), made to answer in the CI
//! container too.
//!
//! That container (`rust:*-alpine`) ships busybox `ps` and `pgrep`, which have
//! no `-p` / `-g`: busybox `pgrep -g` rejects the option and exits 1, which a
//! `pgrep` caller reads as "no such process". And its PID 1 never reaps an
//! orphan, so a killed process stays a zombie that `kill(pid, 0)` still finds.
//! On Linux these probes therefore read `/proc` directly and skip zombies;
//! elsewhere (macOS) they ask `ps` / `pgrep`, which work there.

/// The state letter and process group of `pid`, from `/proc/<pid>/stat`
/// (fields after the command name, which may itself hold a `)`).
#[cfg(target_os = "linux")]
fn stat(pid: u32) -> Option<(char, u32)> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let (_, rest) = stat.rsplit_once(')')?;
    let mut fields = rest.split_whitespace();
    let state = fields.next()?.chars().next()?;
    let _ppid = fields.next()?;
    let pgrp = fields.next()?.parse().ok()?;
    Some((state, pgrp))
}

/// The command line of every live (not zombie) process in group `pgid`;
/// empty when the group is gone. `None` where that cannot be determined.
#[cfg(target_os = "linux")]
pub fn group_members(pgid: u32) -> Option<Vec<String>> {
    let mut members = Vec::new();
    for entry in std::fs::read_dir("/proc").ok()?.filter_map(Result::ok) {
        let Some(pid) = entry.file_name().to_str().and_then(|n| n.parse().ok()) else {
            continue;
        };
        // A process that exited between the listing and this read is gone.
        let Some((state, pgrp)) = stat(pid) else {
            continue;
        };
        if pgrp != pgid || matches!(state, 'Z' | 'X') {
            continue;
        }
        let cmdline = std::fs::read(entry.path().join("cmdline")).unwrap_or_default();
        members.push(
            String::from_utf8_lossy(&cmdline)
                .replace('\0', " ")
                .trim()
                .to_string(),
        );
    }
    Some(members)
}

/// The command line of every process in group `pgid`; empty when the group
/// is gone. `None` where `pgrep` cannot tell.
#[cfg(not(target_os = "linux"))]
pub fn group_members(pgid: u32) -> Option<Vec<String>> {
    let out = std::process::Command::new("pgrep")
        .args(["-g", &pgid.to_string(), "-l", "-f"])
        .output()
        .ok()?;
    match out.status.code() {
        // `<pid> <command line>` per process.
        Some(0) => Some(
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .map(|l| l.split_once(' ').map_or("", |(_, cmd)| cmd).to_string())
                .collect(),
        ),
        Some(1) => Some(Vec::new()),
        _ => None,
    }
}

/// The process group `pid` belongs to; `None` when it is gone or that cannot
/// be determined.
#[cfg(target_os = "linux")]
pub fn pgid_of(pid: u32) -> Option<u32> {
    stat(pid).map(|(_, pgrp)| pgrp)
}

/// The process group `pid` belongs to; `None` when it is gone or `ps` cannot
/// tell.
#[cfg(not(target_os = "linux"))]
pub fn pgid_of(pid: u32) -> Option<u32> {
    let out = std::process::Command::new("ps")
        .args(["-o", "pgid=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

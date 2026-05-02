//! Crash logger. Installs a panic hook that writes a single
//! `~/.rupu/cache/crash-<rfc3339>.log` on panic before letting the
//! default panic behavior run.

use crate::paths;
use std::panic;

pub fn install_panic_hook() {
    let prev = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        if let Err(e) = write_crash_log(&format!("{info}")) {
            eprintln!("rupu: failed to write crash log: {e}");
        }
        prev(info);
    }));
}

fn write_crash_log(info_display: &str) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    let cache = global.join("cache");
    paths::ensure_dir(&cache)?;
    let now = chrono::Utc::now().to_rfc3339();
    let path = cache.join(format!("crash-{now}.log"));
    let body = format!("{info_display}\n\n{}", std::backtrace::Backtrace::force_capture());
    std::fs::write(&path, body)?;
    eprintln!("rupu: crash log written to {}", path.display());
    Ok(())
}

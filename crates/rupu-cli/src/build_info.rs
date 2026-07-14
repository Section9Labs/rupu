//! Build identity embedded at compile time. The release build (see
//! `scripts/gh-build.sh`) exports `RUPU_RELEASE_CHANNEL` + `RUPU_RELEASE_VERSION`;
//! a local/dev build leaves them unset.

/// "beta" | "stable" for a published build; `None` for a dev build.
pub const RELEASE_CHANNEL: Option<&str> = option_env!("RUPU_RELEASE_CHANNEL");

/// The full release version (e.g. "0.35.4-beta" / "0.35.4"); falls back to the
/// crate version for dev builds.
pub const RELEASE_VERSION: &str = match option_env!("RUPU_RELEASE_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};

/// True when this binary was not built by the release tooling.
pub fn is_dev_build() -> bool {
    RELEASE_CHANNEL.is_none()
}

/// Human `--version` suffix, e.g. "0.35.4 (beta)" / "0.35.4 (dev)".
pub fn version_line() -> String {
    format!("{} ({})", RELEASE_VERSION, RELEASE_CHANNEL.unwrap_or("dev"))
}

/// `version_line()`, memoized behind a `'static` reference. clap's
/// `Command::version()` requires `impl IntoResettable<Str>`, which
/// `String` doesn't satisfy (only `&'static str` does) — this computes
/// the line once per process and hands back a `'static` borrow of that
/// single copy, so `rupu --version` can print the channel/version
/// without leaking a fresh allocation on every call.
pub fn version_line_static() -> &'static str {
    static CELL: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    CELL.get_or_init(version_line).as_str()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dev_build_when_env_absent() {
        // Under `cargo test` the release env is unset.
        assert!(is_dev_build());
        assert_eq!(RELEASE_CHANNEL, None);
        assert_eq!(RELEASE_VERSION, env!("CARGO_PKG_VERSION"));
        assert!(version_line().ends_with("(dev)"));
    }
}

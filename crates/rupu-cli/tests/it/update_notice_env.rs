//! `RUPU_NO_UPDATE_CHECK`: unset / empty / `0` / `false` keep the passive
//! update notice on (consistent with `RUPU_NETFLOW_SUBPROCESS=0` meaning
//! "off" rather than "set"); any other value disables it.

use rupu_cli::update_notice::{env_disables_check, should_check};
use std::ffi::OsStr;

#[test]
fn falsy_values_do_not_disable_the_notice() {
    for v in ["", "0", "false", "FALSE", "False", " 0 "] {
        assert!(!env_disables_check(Some(OsStr::new(v))), "{v:?}");
    }
    assert!(!env_disables_check(None));
}

#[test]
fn truthy_values_disable_the_notice() {
    for v in ["1", "true", "yes", "on"] {
        assert!(env_disables_check(Some(OsStr::new(v))), "{v:?}");
    }
}

#[test]
fn zero_leaves_the_tty_notice_on() {
    let disabled = env_disables_check(Some(OsStr::new("0")));
    assert!(should_check(None, disabled, true, false));
}

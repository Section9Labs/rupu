//! The text formats the autoflow reports use (and the CLI's autoflow views
//! with them).

use std::collections::BTreeMap;
use std::time::Duration;

/// `1234567` → `1,234,567`.
pub fn format_count(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out.chars().rev().collect()
}

/// `$0.1234`, with `*` when only part of the spend was priced; `—` when
/// none was.
pub fn format_cost(cost_usd: Option<f64>, partial: bool) -> String {
    match cost_usd {
        Some(value) => format!("${value:.4}{}", if partial { "*" } else { "" }),
        None => "—".into(),
    }
}

/// `value` cut to `max` characters, the last one an ellipsis.
pub fn truncate_text(value: &str, max: usize) -> String {
    let chars = value.chars().collect::<Vec<_>>();
    if chars.len() <= max {
        return value.to_string();
    }
    let keep = max.saturating_sub(1);
    format!("{}…", chars.into_iter().take(keep).collect::<String>())
}

/// `k=v,k=v`, or `-` for none.
pub fn format_inputs(inputs: &BTreeMap<String, String>) -> String {
    if inputs.is_empty() {
        return "-".into();
    }
    inputs
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join(",")
}

/// `4.2s`, `3m 7s`, `1h 2m 3s`. The CLI's printer formats every duration
/// with this.
pub fn format_duration(d: Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        let tenths = d.subsec_millis() / 100;
        format!("{secs}.{tenths}s")
    } else if secs < 3600 {
        let m = secs / 60;
        let s = secs % 60;
        format!("{m}m {s}s")
    } else {
        let h = secs / 3600;
        let m = (secs % 3600) / 60;
        let s = secs % 60;
        format!("{h}h {m}m {s}s")
    }
}

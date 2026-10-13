//! The message-space events (`message`, `injected`, spec F1a §6) and their
//! one-line renderers, so every transcript view says the same thing.

use serde::{Deserialize, Serialize};

/// Which side of a message a transcript is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageDirection {
    Sent,
    Received,
}

/// How a message entered the run: a messaging tool the model called, or a
/// collector delivering it before a model call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageVia {
    Tool,
    Injected,
}

/// One line for a `message`: `sent direct to lynx#2: …` /
/// `received broadcast from lynx#1: …`, the body cut to one short line.
pub fn message_line(
    direction: MessageDirection,
    kind: &str,
    from: &str,
    to: Option<&str>,
    body: &str,
) -> String {
    let flat = body.split_whitespace().collect::<Vec<_>>().join(" ");
    let short: String = flat.chars().take(120).collect();
    let cut = if short.len() < flat.len() { "…" } else { "" };
    match direction {
        MessageDirection::Sent => match to {
            Some(to) => format!("sent {kind} to {to}: {short}{cut}"),
            None => format!("sent {kind}: {short}{cut}"),
        },
        MessageDirection::Received => format!("received {kind} from {from}: {short}{cut}"),
    }
}

/// One line for an `injected`: where the context came from and how much.
pub fn injected_line(source: &str, cadence: &str, content: &str) -> String {
    format!(
        "injected {cadence} context from {source} ({} chars)",
        content.chars().count()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_both_directions() {
        assert_eq!(
            message_line(
                MessageDirection::Sent,
                "direct",
                "lynx#1",
                Some("role:recon"),
                "hi\nthere"
            ),
            "sent direct to role:recon: hi there"
        );
        assert_eq!(
            message_line(
                MessageDirection::Received,
                "broadcast",
                "lynx#1",
                Some("broadcast"),
                "x"
            ),
            "received broadcast from lynx#1: x"
        );
        assert_eq!(
            injected_line("mailbox:lynx#2", "once", "abc"),
            "injected once context from mailbox:lynx#2 (3 chars)"
        );
    }
}

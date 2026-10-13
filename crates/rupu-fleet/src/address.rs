//! Who a message is for (spec F1a §3.2): the `to:` of `msg.send` and the
//! `addressed_to` of `board.post` / `board.directive`.
//!
//! A participant's address is its codename below the message space's root
//! (`lynx#1`, `lynx>otter#1`), with retry attempts dropped, so a retried step
//! keeps its address. In an agentiflow the lead's own address is `lead`.

use std::fmt;

/// The fixed addresses every space has.
pub const OPERATOR: &str = "operator";
pub const BROADCAST: &str = "broadcast";
pub const LEAD: &str = "lead";
pub const PARENT: &str = "parent";

/// A parsed address.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Address {
    /// A participant's address, a full codename, or a bare role: which one
    /// is decided against the roster when it is resolved.
    Named(String),
    /// `role:<agent>`: every live participant running that agent.
    Role(String),
    /// `step:<id>`: every live participant running that workflow step.
    Step(String),
    /// The sender's parent run.
    Parent,
    /// The agentiflow lead.
    Lead,
    /// The operator's inbox.
    Operator,
    /// Every participant, through the broadcast log.
    Broadcast,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid address `{0}`: expected a participant, a codename, role:<agent>, step:<id>, parent, lead, operator or broadcast")]
pub struct AddressError(pub String);

impl Address {
    pub fn parse(s: &str) -> Result<Self, AddressError> {
        let t = s.trim();
        let bad = || AddressError(s.to_string());
        if t.is_empty() || t.chars().any(char::is_whitespace) {
            return Err(bad());
        }
        let prefixed = |p: &str| t.strip_prefix(p).filter(|r| !r.is_empty());
        Ok(match t {
            OPERATOR => Address::Operator,
            BROADCAST => Address::Broadcast,
            LEAD => Address::Lead,
            PARENT => Address::Parent,
            _ if t.starts_with("role:") => Address::Role(prefixed("role:").ok_or_else(bad)?.into()),
            _ if t.starts_with("step:") => Address::Step(prefixed("step:").ok_or_else(bad)?.into()),
            _ => Address::Named(normalize(t)),
        })
    }
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Address::Named(n) => f.write_str(n),
            Address::Role(r) => write!(f, "role:{r}"),
            Address::Step(s) => write!(f, "step:{s}"),
            Address::Parent => f.write_str(PARENT),
            Address::Lead => f.write_str(LEAD),
            Address::Operator => f.write_str(OPERATOR),
            Address::Broadcast => f.write_str(BROADCAST),
        }
    }
}

/// `name` without retry attempts: `lynx#1.2>otter.3` → `lynx#1>otter`. An
/// attempt is a `.<n>` suffix on a segment; a dot anywhere else is kept.
pub fn normalize(name: &str) -> String {
    let strip = |seg: &str| -> String {
        match seg.rsplit_once('.') {
            Some((head, n)) if !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) => {
                head.to_string()
            }
            _ => seg.to_string(),
        }
    };
    match name.split_once('/') {
        Some((crew, path)) => format!(
            "{crew}/{}",
            path.split('>').map(strip).collect::<Vec<_>>().join(">")
        ),
        None => name.split('>').map(strip).collect::<Vec<_>>().join(">"),
    }
}

/// The address of the run named `codename` in a space whose root has
/// already been given the address `root_address` under `root_codename`.
///
/// A run below the root keeps the root's address as a prefix, except below
/// an agentiflow's `lead`, whose units are addressed by what follows it
/// (`lynx#1`, as the launcher has always named them). A run that is not
/// below the root (or has no codename) has no derived address.
pub fn below(root_codename: &str, root_address: &str, codename: &str) -> Option<String> {
    let root = normalize(root_codename);
    let child = normalize(codename);
    let tail = child.strip_prefix(&root)?.strip_prefix('>')?;
    if tail.is_empty() {
        return None;
    }
    Some(if root_address == LEAD {
        tail.to_string()
    } else {
        format!("{root_address}>{tail}")
    })
}

/// The address of a top-level run of a space from its codename: the path
/// after the crew (`cobalt-harbor/lynx#2` → `lynx#2`). `None` for a crew-only
/// or unparseable codename.
pub fn of_codename(codename: &str) -> Option<String> {
    let (_, path) = codename.split_once('/')?;
    let path = normalize(path);
    (!path.is_empty()).then_some(path)
}

/// The directory name of `address`'s mailbox: `#` → `_`, `>` → `.`, anything
/// else outside `[A-Za-z0-9._-]` → `-`. Codename addresses use only role
/// words, digits, `#` and `>`, so two addresses never share a directory.
pub fn mailbox_dir_name(address: &str) -> String {
    address
        .chars()
        .map(|c| match c {
            '#' => '_',
            '>' => '.',
            c if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') => c,
            _ => '-',
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_form() {
        let p = |s: &str| Address::parse(s).unwrap();
        assert_eq!(p("lynx#1"), Address::Named("lynx#1".into()));
        assert_eq!(p("role:recon"), Address::Role("recon".into()));
        assert_eq!(p("step:scan"), Address::Step("scan".into()));
        assert_eq!(p("parent"), Address::Parent);
        assert_eq!(p("lead"), Address::Lead);
        assert_eq!(p("operator"), Address::Operator);
        assert_eq!(p("broadcast"), Address::Broadcast);
        assert_eq!(
            p("cobalt-harbor/heron#412>lynx#3.2"),
            Address::Named("cobalt-harbor/heron#412>lynx#3".into())
        );
        for bad in ["", "role:", "step:", "two words"] {
            assert!(Address::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn addresses_below_a_root() {
        assert_eq!(
            below("c/heron", "lead", "c/heron>lynx#1").as_deref(),
            Some("lynx#1")
        );
        assert_eq!(
            below("c/lynx", "lynx", "c/lynx>otter#1.2").as_deref(),
            Some("lynx>otter#1")
        );
        assert_eq!(below("c/lynx", "lynx", "c/heron>otter#1"), None);
        assert_eq!(of_codename("c/lynx#2.3").as_deref(), Some("lynx#2"));
        assert_eq!(of_codename("c"), None);
    }

    #[test]
    fn mailbox_dirs_never_collide_with_sanitized_legacy_names() {
        assert_eq!(mailbox_dir_name("recon#1"), "recon_1");
        assert_eq!(mailbox_dir_name("recon-1"), "recon-1");
        assert_eq!(mailbox_dir_name("lynx>otter#1"), "lynx.otter_1");
        assert_eq!(mailbox_dir_name("lead"), "lead");
    }
}

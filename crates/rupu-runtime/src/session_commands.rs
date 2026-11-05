//! Shared session slash registry and parser. Never evaluates a shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttachCommand {
    Help,
    Status,
    Detach,
    Quit,
    Cancel,
    Compact,
    Stop,
    History,
    Transcript,
    Runs,
    Coverage,
    Routed(RoutedAttachCommand),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutedAttachCommand {
    pub root: String,
    pub args: Vec<String>,
    pub display: String,
}

pub struct SlashCommand {
    pub name: &'static str,
    pub description: &'static str,
    pub routed: bool,
}

pub const SLASH_COMMANDS: &[SlashCommand] = &[
    SlashCommand {
        name: "cancel",
        description: "cancel the active run",
        routed: false,
    },
    SlashCommand {
        name: "compact",
        description: "summarize older context now (frees the window)",
        routed: false,
    },
    SlashCommand {
        name: "coverage",
        description: "show coverage audit for this session's target",
        routed: false,
    },
    SlashCommand {
        name: "detach",
        description: "leave the live view (keep session running)",
        routed: false,
    },
    SlashCommand {
        name: "help",
        description: "show available commands",
        routed: false,
    },
    SlashCommand {
        name: "history",
        description: "replay prompt history",
        routed: false,
    },
    SlashCommand {
        name: "issues",
        description: "(routed) issues show / list",
        routed: true,
    },
    SlashCommand {
        name: "quit",
        description: "quit and stop the session",
        routed: false,
    },
    SlashCommand {
        name: "runs",
        description: "list runs in this session",
        routed: false,
    },
    SlashCommand {
        name: "session",
        description: "(routed) session show / list",
        routed: true,
    },
    SlashCommand {
        name: "status",
        description: "session status detail",
        routed: false,
    },
    SlashCommand {
        name: "stop",
        description: "stop the worker",
        routed: false,
    },
    SlashCommand {
        name: "transcript",
        description: "show the current transcript",
        routed: false,
    },
    SlashCommand {
        name: "workflow",
        description: "(routed) workflow show / list / show-run",
        routed: true,
    },
];

pub fn parse_attach_command(input: &str) -> Option<AttachCommand> {
    if input.len() > 8192 || input.chars().any(|c| c.is_control() && !c.is_whitespace()) {
        return None;
    }
    let command = input.strip_prefix('/')?.trim();
    match command {
        "help" | "h" | "?" => Some(AttachCommand::Help),
        "status" => Some(AttachCommand::Status),
        "detach" => Some(AttachCommand::Detach),
        "quit" | "exit" => Some(AttachCommand::Quit),
        "cancel" => Some(AttachCommand::Cancel),
        "compact" => Some(AttachCommand::Compact),
        "stop" => Some(AttachCommand::Stop),
        "history" => Some(AttachCommand::History),
        "transcript" => Some(AttachCommand::Transcript),
        "runs" => Some(AttachCommand::Runs),
        "coverage" => Some(AttachCommand::Coverage),
        _ => parse_routed_attach_command(command).map(AttachCommand::Routed),
    }
}

fn parse_routed_attach_command(command: &str) -> Option<RoutedAttachCommand> {
    let argv = tokenize_attach_command(command)?;
    let (root, args) = argv.split_first()?;
    if !matches!(
        root.as_str(),
        "workflow" | "session" | "transcript" | "issues"
    ) {
        return None;
    }
    if !inline_command_is_allowed(root, args) {
        return None;
    }
    Some(RoutedAttachCommand {
        root: root.clone(),
        args: args.to_vec(),
        display: command.to_string(),
    })
}

fn tokenize_attach_command(command: &str) -> Option<Vec<String>> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut chars = command.chars().peekable();
    let mut quote = None;
    let mut started = false;
    while let Some(ch) = chars.next() {
        if !ch.is_whitespace() || quote.is_some() {
            started = true;
        }
        match (quote, ch) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), '\\') => {
                current.push(chars.next()?);
            }
            (Some(_), c) => current.push(c),
            (None, '"' | '\'') => quote = Some(ch),
            (None, '\\') => {
                current.push(chars.next()?);
            }
            (None, c) if c.is_whitespace() => {
                if started {
                    out.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            (None, c) => current.push(c),
        }
    }
    if quote.is_some() {
        return None;
    }
    if started {
        out.push(current);
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

pub fn inline_command_is_allowed(root: &str, args: &[String]) -> bool {
    let Some(subcommand) = args.first().map(|value| value.as_str()) else {
        return false;
    };
    match root {
        "session" => matches!(subcommand, "show" | "list"),
        "transcript" => matches!(subcommand, "show" | "list"),
        "workflow" => matches!(subcommand, "show" | "show-run" | "runs" | "list"),
        "issues" => matches!(subcommand, "show" | "list"),
        _ => false,
    }
}

impl AttachCommand {
    pub fn name(&self) -> &str {
        match self {
            Self::Help => "help",
            Self::Status => "status",
            Self::Detach => "detach",
            Self::Quit => "quit",
            Self::Cancel => "cancel",
            Self::Compact => "compact",
            Self::Stop => "stop",
            Self::History => "history",
            Self::Transcript => "transcript",
            Self::Runs => "runs",
            Self::Coverage => "coverage",
            Self::Routed(c) => &c.root,
        }
    }
    pub fn mutating(&self) -> bool {
        matches!(self, Self::Cancel | Self::Compact | Self::Stop | Self::Quit)
    }
}
pub fn usage(name: &str) -> String {
    match name {
        "workflow" => "/workflow <show NAME|list|show-run RUN|runs> [CLI options]".into(),
        "issues" => "/issues <show ID|list> [CLI options]".into(),
        "session" => "/session <show [ID|current]|list> [CLI options]".into(),
        "transcript" => "/transcript [show [RUN|current|last]|list] [CLI options]".into(),
        _ => format!("/{name}"),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn registry_and_parser_agree() {
        for c in SLASH_COMMANDS {
            let text = format!("/{}{}", c.name, if c.routed { " list" } else { "" });
            assert_eq!(parse_attach_command(&text).unwrap().name(), c.name);
        }
        for text in [
            "plain prompt",
            "/cancel extra",
            "/workflow",
            "/workflow run foo",
            "/session delete foo",
            "/issues list '",
            r"/issues list \",
        ] {
            assert!(parse_attach_command(text).is_none(), "{text}");
        }
    }
    #[test]
    fn quoted_arguments_are_literal() {
        let AttachCommand::Routed(c) =
            parse_attach_command("/issues list --query 'a; $(touch /tmp/no)' --label \"\"")
                .unwrap()
        else {
            panic!()
        };
        assert_eq!(
            c.args,
            ["list", "--query", "a; $(touch /tmp/no)", "--label", ""]
        );
    }
}

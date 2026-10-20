//! The prompts `/mcp` serves (spec M9): a short instruction plus the
//! resources (`rupu://…`, see [`super::resources`]) to read for the task, for
//! clients that offer prompts as commands. The links are written into the
//! text, since a `resource_link` content block is newer than the oldest
//! protocol revision the server speaks.

use async_trait::async_trait;
use rupu_mcp::{PromptArgument, PromptDef, PromptReply, PromptSource};
use std::collections::BTreeMap;

struct Prompt {
    name: &'static str,
    description: &'static str,
    instruction: &'static str,
    links: &'static [(&'static str, &'static str)],
}

const PROMPTS: &[Prompt] = &[
    Prompt {
        name: "author-agent",
        description: "Write a rupu agent file (Markdown with YAML frontmatter) and save it.",
        instruction: "Write a rupu agent: one Markdown file whose YAML frontmatter sets its name, provider, model, tools and limits, and whose body is the system prompt. Keep the tool grant to what the task needs. Save it with `agents.create` (or `agents.save` to overwrite one), then read it back with `agents.get` to check it loads.",
        links: &[
            ("rupu://docs/agent-format", "every frontmatter key"),
            ("rupu://docs/agent-authoring", "how to write a good agent"),
            ("rupu://tools", "the tools a `tools:` grant can name"),
            ("rupu://examples/agents/code-reviewer.md", "a working example"),
        ],
    },
    Prompt {
        name: "author-workflow",
        description: "Write a rupu workflow YAML, validate it and save it.",
        instruction: "Write a rupu workflow: a YAML file of steps that run agents, gates and `action:` tool calls, with fan-out where the work splits. Check it with `workflows.validate` until it parses, then save it with `workflows.create` (or `workflows.save` to overwrite one).",
        links: &[
            ("rupu://docs/workflow-format", "every workflow key"),
            ("rupu://docs/workflow-authoring", "how to write a good workflow"),
            ("rupu://docs/triggers", "running it on a schedule or an SCM event"),
            ("rupu://tools", "the tools an `action:` step can call"),
            ("rupu://examples/workflows/code-review-panel.yaml", "a working example"),
        ],
    },
    Prompt {
        name: "author-agentiflow",
        description: "Write a rupu agentiflow definition: goals, pool, budget and lead.",
        instruction: "Write a rupu agentiflow definition: the goals that say when the run is done (over recorded findings, assets and coverage), the pool of agents and workflows the lead may start, the budget and the lead agent. It runs unattended, so set `scope.authorized: true` only for a pool you would run without approval prompts. It lives at `<project>/.rupu/agentiflows/<name>.yaml` or `~/.rupu/agentiflows/<name>.yaml`.",
        links: &[
            ("rupu://docs/agentiflows", "the definition file and how a run ends"),
            ("rupu://docs/engagement-profiles", "the asset kinds and coverage depths goals count"),
            ("rupu://docs/coverage", "how findings and coverage are recorded"),
            ("rupu://docs/messaging", "how the lead and its units talk"),
        ],
    },
    Prompt {
        name: "triage-findings",
        description: "Triage recorded findings: list, read, tag, starting from a project's query.",
        instruction: "Triage rupu findings. List them with `findings.list` and the query below, read each one that needs a decision with `findings.get` (its report and whether its evidence still matches the files), and record each decision as tags with `findings.tag`. Narrow the query as you go; `facets` in the list answer shows the values in use.",
        links: &[
            ("rupu://query/findings", "the findings query fields"),
            ("rupu://docs/coverage", "findings, tags and the query language"),
            ("rupu://schema/finding-report", "the structured report a finding carries"),
        ],
    },
];

/// The `findings.list` arguments `triage-findings` starts from.
fn triage_start(project: Option<&str>) -> String {
    match project {
        Some(p) => {
            let quoted = format!("\"{}\"", p.replace('\\', "\\\\").replace('"', "\\\""));
            serde_json::json!({ "q": format!("project:{quoted}") }).to_string()
        }
        None => "{}".to_string(),
    }
}

/// The four prompts above, as an MCP prompt source.
pub struct RupuPrompts;

#[async_trait]
impl PromptSource for RupuPrompts {
    fn prompts(&self) -> Vec<PromptDef> {
        PROMPTS
            .iter()
            .map(|p| PromptDef {
                name: p.name.into(),
                description: p.description.into(),
                arguments: if p.name == "triage-findings" {
                    vec![PromptArgument {
                        name: "project".into(),
                        description:
                            "The project whose findings to triage; every project when absent."
                                .into(),
                        required: false,
                    }]
                } else {
                    vec![]
                },
            })
            .collect()
    }

    async fn get(&self, name: &str, args: &BTreeMap<String, String>) -> Option<PromptReply> {
        let p = PROMPTS.iter().find(|p| p.name == name)?;
        let mut text = p.instruction.to_string();
        if p.name == "triage-findings" {
            text.push_str(&format!(
                "\n\nStart with `findings.list` {}.",
                triage_start(args.get("project").map(String::as_str))
            ));
        }
        text.push_str("\n\nRead these resources first (`resources/read`):");
        for (uri, why) in p.links {
            text.push_str(&format!("\n- {uri}: {why}"));
        }
        Some(PromptReply {
            description: p.description.into(),
            text,
        })
    }
}

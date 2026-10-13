//! `skills.load` (spec §3.3): the body of one of the run's granted skills,
//! re-read from disk so an edit since the run started is never hidden.

use crate::descriptor::{Effect, Service, ToolDescriptor};
use crate::output::{failed, ok, req_str};
use crate::tool::{Tool, ToolContext, ToolError, ToolOutput};
use async_trait::async_trait;
use rupu_skills::render::{render_load_result, LoadView, CHANGED_NOTE};
use serde_json::{json, Value};
use std::time::Instant;

pub static LOAD: ToolDescriptor = ToolDescriptor {
    name: "skills.load",
    aliases: &[],
    effect: Effect::Read,
    needs: &[Service::Skills],
    uses: &[],
    wildcard: true,
    description: "Load one of your skills by name. Returns its instructions, its base \
                  directory and its files. Paths inside a skill are relative to its base \
                  directory; read them with read_file and run its scripts with bash. Load a \
                  skill before you start a task its description matches.",
    input_schema: schema,
};

fn schema() -> Value {
    json!({
        "type": "object",
        "required": ["name"],
        "properties": {
            "name": { "type": "string", "description": "The skill's name, as listed under ## Skills." }
        }
    })
}

pub struct SkillsLoadTool;

#[async_trait]
impl Tool for SkillsLoadTool {
    fn descriptor(&self) -> &'static ToolDescriptor {
        &LOAD
    }

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let started = Instant::now();
        let name = req_str(&input, "name")?;
        let skills = ctx
            .services
            .skills
            .as_ref()
            .ok_or_else(|| ToolError::Execution("this run has no skills".into()))?;
        let Some(granted) = skills.get(name).filter(|g| g.model_loadable()) else {
            return Ok(failed(format!(
                "no skill named {name:?} is available to this run; available: {}",
                skills.loadable_names().join(", ")
            ))
            .timed(started));
        };
        // Reading the tree and hashing every file is blocking work.
        let (dir, layer) = (granted.skill.dir.clone(), granted.skill.layer);
        let loaded = tokio::task::spawn_blocking(move || {
            rupu_skills::load_skill(&dir, layer).map(|s| {
                let hash = s.content_hash();
                (s, hash)
            })
        })
        .await
        .unwrap_or_else(|e| std::panic::resume_unwind(e.into_panic()));
        let (fresh, hash) = match loaded {
            Ok(l) => l,
            Err(e) => {
                return Ok(
                    failed(format!("skill {name:?} can no longer be loaded: {e}")).timed(started),
                )
            }
        };
        if fresh.root != granted.skill.root {
            return Ok(failed(format!(
                "skill {name:?} moved on disk since this run started (now at {}); \
                 its files can't be read in this run",
                fresh.root.display()
            ))
            .timed(started));
        }
        let hash = match hash {
            Ok(h) => h,
            Err(e) => {
                return Ok(
                    failed(format!("skill {name:?} can no longer be read: {e}")).timed(started)
                )
            }
        };
        let changed = fresh.stamp != granted.baseline_stamp;
        let files = fresh.files();
        let text = render_load_result(&LoadView {
            name,
            version: hash,
            base_dir: &fresh.root,
            files: &files,
            compatibility: fresh.frontmatter.compatibility.as_deref(),
            changed_note: changed.then_some(CHANGED_NOTE),
            body: &fresh.body,
        });
        let mut out = ok(text);
        out.structured = Some(json!({
            "skill": name,
            "layer": fresh.layer.as_str(),
            "content_hash": hash.hex(),
            "base_dir": fresh.root,
            "changed": changed,
            "allowed_tools": rupu_skills::tools_map::mapped_tools(&fresh.frontmatter.allowed_tools),
        }));
        Ok(out.timed(started))
    }
}

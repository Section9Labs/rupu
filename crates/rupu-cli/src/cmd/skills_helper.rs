//! Hidden `rupu __skills missing|receive` helper: the host side of shipping a
//! placed unit's skills. Writes only into `<RUPU_HOME>/cache/skills`.

use std::io::BufReader;
use std::process::ExitCode;

use anyhow::{anyhow, bail};
use clap::Subcommand;
use rupu_skills::ship::{self, ShippedSkillRef, MAX_BUNDLE_BYTES};
use rupu_skills::Layer;

#[derive(Subcommand, Debug)]
pub enum SkillsHelperAction {
    /// Print the `hash:layer:name` refs (arguments) this host hasn't cached.
    Missing { refs: Vec<String> },
    /// Store the skills framed on stdin (`encode_stream`); print `ok <hash>` per skill.
    Receive,
}

pub async fn handle(action: SkillsHelperAction) -> ExitCode {
    match handle_inner(action) {
        Ok(()) => ExitCode::from(0),
        Err(e) => crate::output::diag::fail(e),
    }
}

fn handle_inner(action: SkillsHelperAction) -> anyhow::Result<()> {
    let cache = ship::cache_dir(&crate::paths::global_dir()?);
    match action {
        SkillsHelperAction::Missing { refs } => {
            let refs = refs
                .iter()
                .map(|r| parse_ref(r))
                .collect::<anyhow::Result<Vec<_>>>()?;
            for r in ship::missing(&cache, &refs) {
                println!("{}:{}:{}", r.content_hash, r.layer, r.name);
            }
        }
        SkillsHelperAction::Receive => {
            let mut stdin = BufReader::new(std::io::stdin().lock());
            let skills = ship::decode_stream(&mut stdin, MAX_BUNDLE_BYTES)
                .map_err(|e| anyhow!(e.to_string()))?;
            for (r, bytes) in &skills {
                ship::store(&cache, r, bytes).map_err(|e| anyhow!(e.to_string()))?;
                println!("ok {}", r.content_hash);
            }
        }
    }
    Ok(())
}

/// `<hash>:<layer>:<name>`; a skill name has no `:`.
fn parse_ref(s: &str) -> anyhow::Result<ShippedSkillRef> {
    let parts: Vec<&str> = s.split(':').collect();
    let [hash, layer, name] = parts[..] else {
        bail!("{s:?} is not `<hash>:<layer>:<name>`");
    };
    let layer = Layer::parse(layer).ok_or_else(|| anyhow!("{s:?}: unknown layer {layer:?}"))?;
    let r = ShippedSkillRef {
        layer,
        name: name.to_owned(),
        content_hash: hash.to_owned(),
    };
    r.validate().map_err(|e| anyhow!(e.to_string()))?;
    Ok(r)
}

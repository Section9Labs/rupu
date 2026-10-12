use std::fs;
use std::path::{Path, PathBuf};

/// Writes `<skills_dir>/<dir>/SKILL.md` naming `name`, plus one script.
pub fn write_skill(skills_dir: &Path, dir: &str, name: &str, description: &str) -> PathBuf {
    let root = skills_dir.join(dir);
    fs::create_dir_all(root.join("scripts")).unwrap();
    fs::write(
        root.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {description}\n---\n\nDo the {name} thing.\n"),
    )
    .unwrap();
    fs::write(root.join("scripts/run.sh"), "#!/bin/sh\necho ok\n").unwrap();
    root
}

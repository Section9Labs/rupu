//! Run-owned approval policy for operator workflows opened from the built-in
//! harness. This is execution policy, not dispatch ancestry. Its presence keeps
//! local steps and their descendants strict across workflow resumes.
use std::{
    fs,
    io::{self, Write},
    path::Path,
};

const FILE: &str = "strict-approval";
const CONTENT: &[u8] = b"rupu-strict-approval-v1\n";

pub fn require_workflow(run_dir: &Path) -> io::Result<()> {
    fs::create_dir_all(run_dir)?;
    let path = run_dir.join(FILE);
    match fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&path)
    {
        Ok(mut file) => {
            file.write_all(CONTENT)?;
            file.sync_all()?;
            fs::File::open(run_dir)?.sync_all()
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            if read_required(&path)? {
                Ok(())
            } else {
                Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    "workflow approval policy disappeared before launch",
                ))
            }
        }
        Err(error) => Err(error),
    }
}

pub fn workflow_required(global: &Path, run_id: &str) -> io::Result<bool> {
    if run_id.is_empty()
        || !run_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid workflow identity for approval policy",
        ));
    }
    read_required(&global.join("runs").join(run_id).join(FILE))
}

fn read_required(path: &Path) -> io::Result<bool> {
    match fs::read(path) {
        Ok(bytes) if bytes == CONTENT => Ok(true),
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "workflow approval policy is unreadable; refusing to discard it",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn workflow_policy_is_durable_and_never_silently_downgrades() {
        let tmp = tempfile::tempdir().unwrap();
        let run = tmp.path().join("runs/run_test");
        assert!(!workflow_required(tmp.path(), "run_test").unwrap());
        require_workflow(&run).unwrap();
        require_workflow(&run).unwrap();
        assert!(workflow_required(tmp.path(), "run_test").unwrap());
        fs::write(run.join(FILE), b"corrupt").unwrap();
        assert!(workflow_required(tmp.path(), "run_test").is_err());
        assert!(require_workflow(&run).is_err());
        assert!(workflow_required(tmp.path(), "../escape").is_err());
    }
}

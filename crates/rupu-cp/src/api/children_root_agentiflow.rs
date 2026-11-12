//! Exact local Agentiflow ownership. This is not a RunStore fallback: the
//! canonical flow record and round-zero lead prologue must independently agree.
use super::*;
use rupu_agentiflow::AgentiflowRecord;
use rupu_launch::ledger::Started;
use std::io::BufRead;
use std::path::Path;

const PROOF_BYTES: usize = 256 * 1024;

fn read(path: &Path, first_line: bool) -> ApiResult<Vec<u8>> {
    let file = crate::api::fs_open::open_anchored_regular_file(path)
        .map_err(|_| unsupported("Agentiflow ownership proof is unavailable"))?;
    let mut bytes = Vec::new();
    let mut bounded = std::io::BufReader::new(file.take((PROOF_BYTES + 1) as u64));
    if first_line {
        bounded
            .read_until(b'\n', &mut bytes)
            .map_err(|_| stale("cannot read Agentiflow lead prologue"))?;
    } else {
        bounded
            .read_to_end(&mut bytes)
            .map_err(|_| stale("cannot read Agentiflow ownership record"))?;
    }
    if bytes.len() > PROOF_BYTES {
        return Err(too_large("Agentiflow ownership proof exceeds 256 KiB"));
    }
    if first_line && !bytes.ends_with(b"\n") {
        return Err(unsupported("Agentiflow lead prologue is incomplete"));
    }
    Ok(bytes)
}

pub(super) fn resolve(owner: &Owner, id: &str) -> ApiResult<Option<(Reader, RootProof)>> {
    if !id.starts_with("af_") {
        return Ok(None);
    }
    let mut reader = Reader::new(owner.global.clone());
    let directory = owner.global.join("agentiflows").join(id);
    let path = directory.join("agentiflow.json");
    let Some(metadata) = reader.checked_metadata(&path)? else {
        return Ok(None);
    };
    if !metadata.is_file() {
        return Err(ApiError::conflict(
            "Agentiflow ownership record is not a regular file",
        ));
    }
    if !id.starts_with("af_") || id.len() > 128 {
        return Err(ApiError::conflict("Agentiflow root identifier mismatch"));
    }
    let directory_identity = Identity::of(
        &reader
            .checked_metadata(&directory)?
            .ok_or_else(|| stale("Agentiflow directory disappeared"))?,
    );
    // A conflicting canonical identity is not resolved by whichever root was
    // checked first. The runs/<flow>/ scratch directory itself is legitimate.
    let conflicts = vec![
        owner.runs.join(id).join("run.json"),
        RunStore::new(owner.runs.clone())
            .archive_root()
            .join(id)
            .join("run.json"),
        owner
            .global
            .join("transcripts")
            .join(format!("{id}.meta.json")),
        owner
            .global
            .join("transcripts/archive")
            .join(format!("{id}.meta.json")),
    ];
    for conflict in &conflicts {
        match fs::symlink_metadata(conflict) {
            Ok(_) => {
                return Err(ApiError::conflict(
                    "Agentiflow root has conflicting canonical ownership",
                ))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(_) => {
                return Err(ApiError::conflict(
                    "cannot exclude conflicting Agentiflow ownership",
                ))
            }
        }
    }
    let bytes = read(&path, false)?;
    let record: AgentiflowRecord = serde_json::from_slice(&bytes)
        .map_err(|_| ApiError::conflict("invalid canonical Agentiflow record"))?;
    if record.id != id {
        return Err(ApiError::conflict("canonical Agentiflow identity mismatch"));
    }
    let raw: serde_json::Value = serde_json::from_slice(&bytes).expect("typed record parsed");
    if !raw["host_id"].is_null() && raw["host_id"].as_str() != Some("local") {
        return Err(ApiError::conflict("Agentiflow ownership is not local"));
    }
    let lead = directory.join("lead/transcript.r0.jsonl");
    let lead_metadata = reader
        .checked_metadata(&lead)?
        .ok_or_else(|| unsupported("Agentiflow has no recorded round-zero lead prologue"))?;
    if !lead_metadata.is_file() {
        return Err(ApiError::conflict("Agentiflow lead is not a regular file"));
    }
    let line = read(&lead, true)?;
    let rupu_transcript::Event::RunStart {
        run_id,
        workspace_id,
        ..
    } = serde_json::from_slice::<rupu_transcript::Event>(&line)
        .map_err(|_| ApiError::conflict("invalid Agentiflow lead prologue"))?
    else {
        return Err(ApiError::conflict(
            "Agentiflow lead must begin with run_start",
        ));
    };
    valid_id(&workspace_id)?;
    let prologue: serde_json::Value = serde_json::from_slice(&line).expect("typed event parsed");
    if run_id != id
        || [&prologue["host_id"], &prologue["data"]["host_id"]]
            .iter()
            .any(|v| !v.is_null() && v.as_str() != Some("local"))
        || (!prologue["run_id"].is_null() && prologue["run_id"].as_str() != Some(id))
        || (!prologue["workspace_id"].is_null()
            && prologue["workspace_id"].as_str() != Some(&workspace_id))
        || (!raw["workspace_id"].is_null() && raw["workspace_id"].as_str() != Some(&workspace_id))
    {
        return Err(ApiError::conflict(
            "Agentiflow lead owner or project mismatch",
        ));
    }
    let ledger = reader
        .checked_metadata(&directory.join("children.jsonl"))?
        .ok_or_else(|| unsupported("Agentiflow has no canonical children ledger"))?;
    if !ledger.is_file() {
        return Err(ApiError::conflict(
            "Agentiflow children ledger is not regular",
        ));
    }
    reader.evidence.bytes_read += bytes.len() + line.len();
    let proof = RootProof {
        kind: "agentiflow",
        project_id: Some(workspace_id),
        transcript_path: Some(lead.clone()),
        anchors: vec![
            Anchor {
                path,
                identity: Some(Identity::of(&metadata)),
                digest: Sha256::digest(&bytes).to_vec(),
            },
            Anchor {
                path: lead,
                identity: Some(Identity::of(&lead_metadata)),
                digest: Sha256::digest(&line).to_vec(),
            },
        ],
        directory: Some((directory, directory_identity)),
        conflicts,
    };
    proof.validate()?;
    reader.root = owner.global.join("agentiflows");
    Ok(Some((reader, proof)))
}

pub(super) fn validate(proof: &RootProof) -> ApiResult<()> {
    for (index, anchor) in proof.anchors.iter().enumerate() {
        let bytes = read(&anchor.path, index == 1)
            .map_err(|_| stale("Agentiflow ownership proof changed"))?;
        if Sha256::digest(bytes).as_slice() != anchor.digest {
            return Err(stale("Agentiflow ownership proof changed"));
        }
    }
    Ok(())
}

pub(in super::super) fn validate_edge(row: &Started, directory: &Path) -> ApiResult<()> {
    // The ledger location establishes the parent; an explicit contradictory
    // tree must not be silently re-parented. Old absent tree/round stay absent.
    if row.tree.as_deref().is_some_and(|tree| tree != directory) {
        return Err(ApiError::conflict(
            "Agentiflow child ledger records a different root",
        ));
    }
    valid_id(&row.run_id)?;
    if let Some(path) = &row.transcript {
        if !path.is_absolute()
            || path.components().any(|c| {
                !matches!(
                    c,
                    std::path::Component::RootDir | std::path::Component::Normal(_)
                )
            })
        {
            return Err(ApiError::conflict(
                "Agentiflow child transcript locator is invalid",
            ));
        }
    }
    Ok(())
}

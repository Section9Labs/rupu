//! Private CP↔worker protocol; never an operator HTTP or model-tool API.
use rupu_tools::approval::{ApprovalCompletion, ApprovalOwner, CallApprovalRequest};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
use std::io;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const MAX_FRAME_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
// Frames are bounded and processed individually; retain the wire DTO construction API.
#[allow(clippy::large_enum_variant)]
pub enum ClientFrame {
    Bind {
        session_id: String,
        worker_pid: u32,
    },
    CheckOwner {
        owner: ApprovalOwner,
    },
    Request {
        call: CallApprovalRequest,
    },
    Complete {
        approval_id: String,
        intent_fingerprint: String,
        outcome: ApprovalCompletion,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ServerFrame {
    Bound {
        instance_id: String,
        broker_epoch: String,
        worker_generation: u64,
    },
    Ready {},
    Pending {
        event: Value,
    },
    Denied {
        reason: String,
    },
    Reserved {
        approval_id: String,
        intent_fingerprint: String,
    },
    Recorded {},
    Refused {
        reason: String,
    },
}

pub async fn write_frame<W: AsyncWrite + Unpin, T: Serialize>(
    writer: &mut W,
    value: &T,
) -> io::Result<()> {
    let bytes =
        serde_json::to_vec(value).map_err(|_| io::Error::other("invalid approval frame"))?;
    if bytes.len() > MAX_FRAME_BYTES {
        return Err(io::Error::other("approval frame exceeds bound"));
    }
    writer.write_u32(bytes.len() as u32).await?;
    writer.write_all(&bytes).await?;
    writer.flush().await
}

pub async fn read_frame<R: AsyncRead + Unpin, T: DeserializeOwned>(
    reader: &mut R,
) -> io::Result<T> {
    let size = reader.read_u32().await? as usize;
    if size == 0 || size > MAX_FRAME_BYTES {
        return Err(io::Error::other("invalid approval frame size"));
    }
    let mut bytes = vec![0; size];
    reader.read_exact(&mut bytes).await?;
    serde_json::from_slice(&bytes).map_err(|_| io::Error::other("invalid approval frame"))
}

/// Exact trusted registry identity (no credential values); use before each
/// remote approval reservation as well as at worker assembly.
pub fn registry_fingerprint(registry: &rupu_config::mcp::Registry) -> String {
    use sha2::{Digest, Sha256};
    fn sorted(v: &Value) -> Value {
        match v {
            Value::Object(o) => {
                let ordered: std::collections::BTreeMap<_, _> = o.iter().collect();
                Value::Object(
                    ordered
                        .into_iter()
                        .map(|(k, v)| (k.clone(), sorted(v)))
                        .collect(),
                )
            }
            Value::Array(a) => Value::Array(a.iter().map(sorted).collect()),
            _ => v.clone(),
        }
    }
    let value = serde_json::to_value(registry).expect("validated registry JSON");
    format!(
        "sha256:{:x}",
        Sha256::digest(serde_json::to_vec(&sorted(&value)).expect("JSON value"))
    )
}

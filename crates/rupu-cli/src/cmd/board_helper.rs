//! The hidden `rupu __board <id>`: what an SSH coordinator runs on a remote
//! host to read a run's board. Its stdout is a contract with `rupu-cp`'s SSH
//! connector (`rupu_cp::host::ssh::board_argv`):
//!
//! - success prints one JSON object, the `BoardView`
//!   (`rupu_runtime::services::board`) of the message space `<id>` names, and
//!   exits 0;
//! - any failure prints `{"error": {"code", "message"}}`
//!   (`MessageErrorReply`; code `not_found` or `internal`) and exits 3.
//!
//! `<id>` is exact (no fragment resolution). Nothing else is ever printed on
//! stdout.

use std::path::Path;
use std::process::ExitCode;

use rupu_runtime::services::board::board_view;
use rupu_runtime::services::operator::{
    MessageError, MessageErrorCode, MessageErrorReply, MessageRoot,
};

use crate::paths;

/// The exit status of a failed `rupu __board`.
const HELPER_FAILED: u8 = 3;

pub fn handle(id: &str) -> ExitCode {
    let internal =
        |e: &dyn std::fmt::Display| MessageError::new(MessageErrorCode::Internal, e.to_string());
    let reply = paths::global_dir()
        .map_err(|e| internal(&e))
        .and_then(|global| view(&global, id))
        .and_then(|view| serde_json::to_string(&view).map_err(|e| internal(&e)));
    match reply {
        Ok(json) => {
            println!("{json}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            match serde_json::to_string(&MessageErrorReply { error }) {
                Ok(json) => println!("{json}"),
                Err(_) => println!(r#"{{"error":{{"code":"internal","message":"serialize"}}}}"#),
            }
            ExitCode::from(HELPER_FAILED)
        }
    }
}

fn view(global: &Path, id: &str) -> Result<rupu_runtime::services::board::BoardView, MessageError> {
    let root = MessageRoot::resolve(global, id).map_err(|e| MessageError::from(&e))?;
    board_view(&root.space, chrono::Utc::now()).map_err(|e| {
        MessageError::new(
            MessageErrorCode::Internal,
            format!("`{id}`: the board is unreadable: {e}"),
        )
    })
}

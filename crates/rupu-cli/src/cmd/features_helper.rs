//! Hidden `rupu __features` helper — prints this build's host features
//! (`rupu_cp::node::protocol::FeaturesReport`) as one JSON object. An SSH
//! coordinator runs it on the remote before a command the remote may predate
//! (e.g. `workflow resume --if-unfinished`, chosen in the same remote
//! command); `/api/host/info` serves the same list.

use std::process::ExitCode;

pub fn handle() -> ExitCode {
    match serde_json::to_string(&rupu_cp::node::protocol::FeaturesReport::current()) {
        Ok(report) => {
            println!("{report}");
            ExitCode::SUCCESS
        }
        Err(e) => crate::output::diag::fail(anyhow::anyhow!("serialize features: {e}")),
    }
}

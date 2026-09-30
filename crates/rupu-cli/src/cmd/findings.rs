//! `rupu findings` — the finding report contract (and, in a later plan,
//! report exports). Thin: delegates to `rupu_coverage::report`.

use clap::Subcommand;
use std::process::ExitCode;

#[derive(Debug, Subcommand)]
pub enum Action {
    /// Print the finding report JSON Schema embedded in this build.
    Schema {
        /// Print the simplified copy used in tool definitions instead.
        #[arg(long)]
        advertised: bool,
    },
}

pub async fn handle(action: Action) -> ExitCode {
    async fn schema_cmd(advertised: bool) -> anyhow::Result<()> {
        let v = if advertised {
            rupu_coverage::report::schema::advertised_schema()
        } else {
            rupu_coverage::report::schema::canonical_schema()
        };
        println!("{}", serde_json::to_string_pretty(&v)?);
        Ok(())
    }

    let result = match action {
        Action::Schema { advertised } => schema_cmd(advertised).await,
    };
    match result {
        Ok(()) => ExitCode::from(0),
        Err(e) => crate::output::diag::fail(e),
    }
}

//! Step-prompt template rendering with minijinja. Real impl in Task 10.

use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum RenderError {
    #[error("minijinja: {0}")]
    Mini(String),
}

#[derive(Debug, Serialize)]
pub struct StepContext;

pub fn render_step_prompt(_template: &str, _ctx: &StepContext) -> Result<String, RenderError> {
    todo!("render_step_prompt lands in Task 10")
}

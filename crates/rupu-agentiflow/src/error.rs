use thiserror::Error;

#[derive(Debug, Error)]
pub enum AgentiflowError {
    #[error("agentiflow YAML parse error: {0}")]
    Yaml(#[from] serde_yaml::Error),
    #[error("invalid agentiflow definition: {0}")]
    Invalid(String),
    #[error("unknown engagement profile(s): {0}")]
    UnknownProfile(String),
}

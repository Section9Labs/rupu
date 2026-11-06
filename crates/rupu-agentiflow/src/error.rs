use thiserror::Error;

#[derive(Debug, Error)]
pub enum AgentiflowError {
    #[error("agentiflow YAML parse error: {0}")]
    Yaml(#[from] serde_yaml::Error),
    #[error("invalid agentiflow definition: {0}")]
    Invalid(String),
    #[error("unknown engagement profile(s): {0}")]
    UnknownProfile(String),
    /// The engagement-profile registry could not be built (a profile file
    /// that does not parse fails it closed).
    #[error("engagement profiles: {0}")]
    Profiles(String),
    /// A run-directory operation failed (creating the directory, persisting
    /// `agentiflow.json`, building the lead driver).
    #[error("agentiflow run I/O error: {0}")]
    Io(#[from] std::io::Error),
}

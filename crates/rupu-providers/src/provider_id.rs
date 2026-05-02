use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// Identifies which OAuth LLM provider to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderId {
    #[default]
    Anthropic,
    OpenaiCodex,
    GoogleGeminiCli,
    GoogleAntigravity,
    GithubCopilot,
}

impl ProviderId {
    /// All known provider IDs, for iteration.
    pub const ALL: &[ProviderId] = &[
        ProviderId::Anthropic,
        ProviderId::OpenaiCodex,
        ProviderId::GoogleGeminiCli,
        ProviderId::GoogleAntigravity,
        ProviderId::GithubCopilot,
    ];

    /// The key used in auth.json for this provider.
    pub fn auth_key(&self) -> &'static str {
        match self {
            ProviderId::Anthropic => "anthropic",
            ProviderId::OpenaiCodex => "openai-codex",
            ProviderId::GoogleGeminiCli => "google-gemini-cli",
            ProviderId::GoogleAntigravity => "google-antigravity",
            ProviderId::GithubCopilot => "github-copilot",
        }
    }

    /// Environment variable fallback name for this provider's API key.
    pub fn env_var_name(&self) -> &'static str {
        match self {
            ProviderId::Anthropic => "ANTHROPIC_API_KEY",
            ProviderId::OpenaiCodex => "OPENAI_API_KEY",
            ProviderId::GoogleGeminiCli => "GOOGLE_GEMINI_API_KEY",
            ProviderId::GoogleAntigravity => "GOOGLE_ANTIGRAVITY_API_KEY",
            ProviderId::GithubCopilot => "GITHUB_TOKEN",
        }
    }
}

impl fmt::Display for ProviderId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.auth_key())
    }
}

impl FromStr for ProviderId {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "anthropic" => Ok(ProviderId::Anthropic),
            "openai-codex" | "openai" => Ok(ProviderId::OpenaiCodex),
            "google-gemini-cli" | "gemini-cli" | "gemini" => Ok(ProviderId::GoogleGeminiCli),
            "google-antigravity" | "antigravity" => Ok(ProviderId::GoogleAntigravity),
            "github-copilot" | "copilot" => Ok(ProviderId::GithubCopilot),
            _ => Err(format!("unknown provider: {s}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serde_roundtrip() {
        for id in ProviderId::ALL {
            let json = serde_json::to_string(id).unwrap();
            let parsed: ProviderId = serde_json::from_str(&json).unwrap();
            assert_eq!(*id, parsed);
        }
    }

    #[test]
    fn test_serde_kebab_case() {
        let json = serde_json::to_string(&ProviderId::OpenaiCodex).unwrap();
        assert_eq!(json, "\"openai-codex\"");

        let json = serde_json::to_string(&ProviderId::GoogleGeminiCli).unwrap();
        assert_eq!(json, "\"google-gemini-cli\"");
    }

    #[test]
    fn test_display_matches_auth_key() {
        for id in ProviderId::ALL {
            assert_eq!(id.to_string(), id.auth_key());
        }
    }

    #[test]
    fn test_from_str_exact() {
        assert_eq!(
            ProviderId::from_str("anthropic").unwrap(),
            ProviderId::Anthropic
        );
        assert_eq!(
            ProviderId::from_str("openai-codex").unwrap(),
            ProviderId::OpenaiCodex
        );
        assert_eq!(
            ProviderId::from_str("github-copilot").unwrap(),
            ProviderId::GithubCopilot
        );
    }

    #[test]
    fn test_from_str_aliases() {
        assert_eq!(
            ProviderId::from_str("openai").unwrap(),
            ProviderId::OpenaiCodex
        );
        assert_eq!(
            ProviderId::from_str("gemini").unwrap(),
            ProviderId::GoogleGeminiCli
        );
        assert_eq!(
            ProviderId::from_str("copilot").unwrap(),
            ProviderId::GithubCopilot
        );
    }

    #[test]
    fn test_from_str_unknown() {
        assert!(ProviderId::from_str("unknown").is_err());
        assert!(ProviderId::from_str("").is_err());
    }

    #[test]
    fn test_default_is_anthropic() {
        assert_eq!(ProviderId::default(), ProviderId::Anthropic);
    }

    #[test]
    fn test_env_var_name() {
        assert_eq!(ProviderId::Anthropic.env_var_name(), "ANTHROPIC_API_KEY");
        assert_eq!(ProviderId::OpenaiCodex.env_var_name(), "OPENAI_API_KEY");
        assert_eq!(
            ProviderId::GoogleGeminiCli.env_var_name(),
            "GOOGLE_GEMINI_API_KEY"
        );
        assert_eq!(
            ProviderId::GoogleAntigravity.env_var_name(),
            "GOOGLE_ANTIGRAVITY_API_KEY"
        );
        assert_eq!(ProviderId::GithubCopilot.env_var_name(), "GITHUB_TOKEN");
    }
}

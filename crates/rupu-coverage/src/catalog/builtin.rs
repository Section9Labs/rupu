use crate::catalog::parse::{parse_template_str, ParseError};
use crate::catalog::types::Template;

/// Static map of template name → bundled YAML body.
const BUILTIN_TEMPLATES: &[(&str, &str)] = &[
    (
        "owasp-top10-2021",
        include_str!("../../templates/concerns/owasp-top10-2021.yaml"),
    ),
    (
        "owasp-api-top10-2023",
        include_str!("../../templates/concerns/owasp-api-top10-2023.yaml"),
    ),
    (
        "cwe-top25-2023",
        include_str!("../../templates/concerns/cwe-top25-2023.yaml"),
    ),
    (
        "stride",
        include_str!("../../templates/concerns/stride.yaml"),
    ),
    (
        "secrets-in-source",
        include_str!("../../templates/concerns/secrets-in-source.yaml"),
    ),
    (
        "code-smells",
        include_str!("../../templates/concerns/code-smells.yaml"),
    ),
    (
        "web-security-default",
        include_str!("../../templates/concerns/web-security-default.yaml"),
    ),
    (
        "api-security-default",
        include_str!("../../templates/concerns/api-security-default.yaml"),
    ),
];

pub fn resolve_builtin(name: &str) -> Option<Result<Template, ParseError>> {
    BUILTIN_TEMPLATES
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, body)| parse_template_str(body, &format!("builtin:{name}")))
}

pub fn builtin_names() -> impl Iterator<Item = &'static str> {
    BUILTIN_TEMPLATES.iter().map(|(n, _)| *n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_builtin_resolves_to_template_with_matching_name() {
        for name in builtin_names() {
            let resolved = resolve_builtin(name).expect("name exists").expect("parses");
            assert_eq!(resolved.name, name, "template body's name field must match registry key");
        }
    }

    #[test]
    fn unknown_template_returns_none() {
        assert!(resolve_builtin("definitely-not-real").is_none());
    }
}

use sha2::{Digest, Sha256};

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    hex_lower(&h.finalize())
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// First whitespace-delimited token of a `shasum`-style sidecar, lowercased.
pub fn parse_sha256_sidecar(text: &str) -> Option<String> {
    let tok = text.split_whitespace().next()?;
    if tok.len() == 64 && tok.chars().all(|c| c.is_ascii_hexdigit()) {
        Some(tok.to_ascii_lowercase())
    } else {
        None
    }
}

pub fn verify_checksum(bytes: &[u8], sidecar_text: &str) -> Result<(), crate::UpdateError> {
    let expected = parse_sha256_sidecar(sidecar_text)
        .ok_or_else(|| crate::UpdateError::Parse("malformed sha256 sidecar".into()))?;
    let actual = sha256_hex(bytes);
    if actual == expected {
        Ok(())
    } else {
        Err(crate::UpdateError::Checksum { expected, actual })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verifies_matching_checksum() {
        let data = b"hello rupu";
        let side = format!("{}  rupu-darwin-arm64", sha256_hex(data));
        assert!(verify_checksum(data, &side).is_ok());
    }
    #[test]
    fn rejects_mismatch() {
        let side = format!("{}  x", sha256_hex(b"other"));
        let err = verify_checksum(b"hello rupu", &side).unwrap_err();
        assert!(matches!(err, crate::UpdateError::Checksum { .. }));
    }
    #[test]
    fn parses_bare_hex_sidecar() {
        assert_eq!(
            parse_sha256_sidecar(&"AB".repeat(32)).unwrap(),
            "ab".repeat(32)
        );
    }
}

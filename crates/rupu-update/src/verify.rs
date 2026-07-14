use sha2::{Digest, Sha256};

/// Sanity + authenticity checks run on the downloaded bytes *after* the
/// sha256 matches and *before* the atomic swap (spec §6.2 step 5): Mach-O
/// shape, `codesign --verify --strict`, quarantine strip, and an optional
/// `--version` smoke test. Behind a trait so `flow::install`'s tests (whose
/// fake payloads are arbitrary bytes, not signed binaries) can inject
/// [`NoopBinaryCheck`] instead of the real [`CodesignCheck`].
pub trait BinaryCheck {
    fn verify(&self, bytes: &[u8]) -> Result<(), crate::UpdateError>;
}

/// Accepts any bytes. Used by tests and anywhere a real signature check
/// isn't meaningful (the fake payloads `flow.rs`'s tests exercise aren't
/// signed executables).
pub struct NoopBinaryCheck;

impl BinaryCheck for NoopBinaryCheck {
    fn verify(&self, _bytes: &[u8]) -> Result<(), crate::UpdateError> {
        Ok(())
    }
}

/// True if `bytes` starts with a Mach-O (32/64-bit, either byte order) or
/// fat-binary magic number. Checked independently of OS so it stays unit
/// testable everywhere; the OS gate lives in [`CodesignCheck::verify`].
fn is_macho(bytes: &[u8]) -> bool {
    if bytes.len() < 4 {
        return false;
    }
    const MAGIC_64: u32 = 0xFEED_FACF;
    const CIGAM_64: u32 = 0xCFFA_EDFE;
    const FAT_MAGIC: u32 = 0xCAFE_BABE;
    let word = [bytes[0], bytes[1], bytes[2], bytes[3]];
    let be = u32::from_be_bytes(word);
    let le = u32::from_le_bytes(word);
    matches!(be, MAGIC_64 | CIGAM_64 | FAT_MAGIC) || matches!(le, MAGIC_64 | CIGAM_64 | FAT_MAGIC)
}

/// Real macOS binary check: Mach-O magic → `codesign --verify --strict` →
/// best-effort quarantine strip → `<temp> --version` smoke test. No-op on
/// every other OS (there is no codesign/quarantine equivalent there), so
/// the field is gated at runtime rather than behind `cfg(target_os)` —
/// that keeps the type constructible (and its non-macOS behavior
/// testable) on any host.
pub struct CodesignCheck;

impl BinaryCheck for CodesignCheck {
    fn verify(&self, bytes: &[u8]) -> Result<(), crate::UpdateError> {
        if std::env::consts::OS != "macos" {
            return Ok(());
        }
        if !is_macho(bytes) {
            return Err(crate::UpdateError::Install(
                "downloaded file is not a Mach-O executable".into(),
            ));
        }

        let unique = format!(
            "rupu-update-verify-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        );
        let tmp = std::env::temp_dir().join(unique);

        let result = (|| -> Result<(), crate::UpdateError> {
            std::fs::write(&tmp, bytes)?;
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = std::fs::metadata(&tmp)?.permissions();
                perms.set_mode(0o755);
                std::fs::set_permissions(&tmp, perms)?;
            }

            let status = std::process::Command::new("codesign")
                .arg("--verify")
                .arg("--strict")
                .arg(&tmp)
                .status()?;
            if !status.success() {
                return Err(crate::UpdateError::Install(format!(
                    "code signature verification failed: codesign exited with {status}"
                )));
            }

            // Best-effort: quarantine may simply not be set on a
            // freshly-written temp file; ignore any failure here.
            let _ = std::process::Command::new("xattr")
                .arg("-d")
                .arg("com.apple.quarantine")
                .arg(&tmp)
                .status();

            let status = std::process::Command::new(&tmp).arg("--version").status()?;
            if !status.success() {
                return Err(crate::UpdateError::Install(format!(
                    "downloaded binary failed its own `--version` check: exited with {status}"
                )));
            }
            Ok(())
        })();

        let _ = std::fs::remove_file(&tmp);
        result
    }
}

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

    #[test]
    fn noop_binary_check_accepts_anything() {
        assert!(NoopBinaryCheck.verify(b"NOTBINARY").is_ok());
        assert!(NoopBinaryCheck.verify(b"").is_ok());
    }

    #[test]
    fn is_macho_rejects_non_macho_bytes() {
        assert!(!is_macho(b"NOTBINARY"));
        assert!(!is_macho(b"ab"));
        assert!(!is_macho(b""));
    }

    #[test]
    fn is_macho_accepts_known_magics() {
        assert!(is_macho(&0xFEED_FACFu32.to_be_bytes()));
        assert!(is_macho(&0xCFFA_EDFEu32.to_be_bytes()));
        assert!(is_macho(&0xCAFE_BABEu32.to_be_bytes()));
    }

    #[test]
    fn codesign_check_rejects_non_macho_bytes() {
        // On macOS this hits the Mach-O magic check and fails fast
        // (never shells out to `codesign`); on any other OS the whole
        // check is a runtime no-op, per spec §6.2 (Mach-O/codesign are
        // macOS-only concepts).
        let result = CodesignCheck.verify(b"NOTBINARY");
        if std::env::consts::OS == "macos" {
            assert!(matches!(result, Err(crate::UpdateError::Install(_))));
        } else {
            assert!(result.is_ok());
        }
    }
}

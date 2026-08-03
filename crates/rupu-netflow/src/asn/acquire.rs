//! Automatic acquisition of the ASN table.
//!
//! The operator never has to run a command. Two triggers drive this:
//! the `rupu cp serve` sweep loop, and any netflow read that finds the
//! DB missing or stale (Plan 2). Failure is always best-effort — log
//! once, never block a read, never block a request.

use crate::asn::table::AsnTable;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

#[derive(Debug, thiserror::Error)]
pub enum AsnError {
    #[error("asn source returned HTTP {0}")]
    Status(u16),
    #[error("asn source request failed: {0}")]
    Transport(String),
    #[error("asn table io: {0}")]
    Io(#[from] std::io::Error),
    /// The body decompressed and parsed, but yielded no usable ranges.
    /// `compact_from_tsv` skips malformed rows rather than failing, so a
    /// gzipped error page or a reformatted upstream file parses
    /// "successfully" into an empty table. Replacing a working table
    /// with that would silently destroy enrichment.
    #[error("asn source parsed to an empty table; refusing to replace")]
    Empty,
}

/// `~/.rupu/netflow/asn.db`. `None` when the home directory is unknown.
pub fn asn_db_path() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|h| h.join(".rupu").join("netflow").join("asn.db"))
}

/// True when the table is absent, unreadable, or older than the interval.
/// An interval of 0 means always refresh.
pub fn is_stale(path: &Path, max_age_days: u64) -> bool {
    if max_age_days == 0 {
        return true;
    }
    let Ok(meta) = std::fs::metadata(path) else {
        return true;
    };
    let Ok(modified) = meta.modified() else {
        return true;
    };
    let max_age = Duration::from_secs(max_age_days.saturating_mul(86_400));
    SystemTime::now()
        .duration_since(modified)
        .map(|age| age > max_age)
        .unwrap_or(true)
}

/// Decompress a gzipped iptoasn TSV into a compacted table.
pub fn ingest_gz<R: Read>(reader: R) -> std::io::Result<AsnTable> {
    let decoder = flate2::read::GzDecoder::new(reader);
    AsnTable::compact_from_tsv(BufReader::new(decoder))
}

/// Download, decompress and atomically replace the table at `dest`.
///
/// On any failure the existing table is left untouched — a failed
/// refresh must never degrade enrichment that already works.
#[cfg(feature = "http")]
pub async fn refresh(
    url: &str,
    dest: &Path,
    client: &reqwest_middleware::ClientWithMiddleware,
) -> Result<(), AsnError> {
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| AsnError::Transport(e.to_string()))?;

    let status = resp.status();
    if !status.is_success() {
        return Err(AsnError::Status(status.as_u16()));
    }

    let bytes = resp
        .bytes()
        .await
        .map_err(|e| AsnError::Transport(e.to_string()))?;

    let table = ingest_gz(std::io::Cursor::new(bytes))?;
    // Guard BEFORE the atomic replace. Parsing cannot fail, so "Ok" is
    // not evidence the body was really an ASN table.
    if table.is_empty() {
        return Err(AsnError::Empty);
    }
    table.write(dest)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    const TSV: &str = "1.0.0.0\t1.0.0.255\t13335\tUS\tCLOUDFLARENET\n";

    fn gzipped(body: &str) -> Vec<u8> {
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        enc.write_all(body.as_bytes()).unwrap();
        enc.finish().unwrap()
    }

    #[test]
    fn ingest_gz_decompresses_and_builds_the_table() {
        let table = ingest_gz(std::io::Cursor::new(gzipped(TSV))).unwrap();
        assert_eq!(table.lookup("1.0.0.7".parse().unwrap()).unwrap().asn, 13335);
    }

    #[test]
    fn a_missing_db_is_stale() {
        let tmp = tempfile::TempDir::new().unwrap();
        assert!(is_stale(&tmp.path().join("absent.db"), 7));
    }

    #[test]
    fn a_freshly_written_db_is_not_stale() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("asn.db");
        std::fs::write(&path, b"{}").unwrap();
        assert!(!is_stale(&path, 7));
    }

    #[test]
    fn a_zero_day_interval_makes_everything_stale() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("asn.db");
        std::fs::write(&path, b"{}").unwrap();
        assert!(is_stale(&path, 0));
    }

    #[cfg(feature = "http")]
    #[tokio::test]
    async fn refresh_downloads_decompresses_and_writes_the_table() {
        let server = httpmock::MockServer::start_async().await;
        let body = gzipped(TSV);
        let mock = server
            .mock_async(|when, then| {
                when.method(httpmock::Method::GET).path("/ip2asn.tsv.gz");
                then.status(200).body(body.clone());
            })
            .await;

        let tmp = tempfile::TempDir::new().unwrap();
        let dest = tmp.path().join("asn.db");
        let client = reqwest_middleware::ClientBuilder::new(reqwest::Client::new()).build();

        refresh(&server.url("/ip2asn.tsv.gz"), &dest, &client)
            .await
            .unwrap();

        mock.assert_async().await;
        let table = AsnTable::load(&dest).unwrap();
        assert_eq!(table.lookup("1.0.0.7".parse().unwrap()).unwrap().asn, 13335);
    }

    #[cfg(feature = "http")]
    #[tokio::test]
    async fn refresh_leaves_an_existing_db_intact_when_the_source_fails() {
        let server = httpmock::MockServer::start_async().await;
        server
            .mock_async(|when, then| {
                when.method(httpmock::Method::GET).path("/bad");
                then.status(500);
            })
            .await;

        let tmp = tempfile::TempDir::new().unwrap();
        let dest = tmp.path().join("asn.db");
        AsnTable::default().write(&dest).unwrap();
        let before = std::fs::read(&dest).unwrap();

        let client = reqwest_middleware::ClientBuilder::new(reqwest::Client::new()).build();
        let err = refresh(&server.url("/bad"), &dest, &client).await;

        assert!(err.is_err());
        assert_eq!(std::fs::read(&dest).unwrap(), before);
    }

    #[cfg(feature = "http")]
    #[tokio::test]
    async fn a_garbage_body_never_replaces_a_working_table() {
        let server = httpmock::MockServer::start_async().await;
        let body = gzipped("<html>502 Bad Gateway</html>\n");
        server
            .mock_async(|when, then| {
                when.method(httpmock::Method::GET).path("/garbage");
                then.status(200).body(body.clone());
            })
            .await;

        let tmp = tempfile::TempDir::new().unwrap();
        let dest = tmp.path().join("asn.db");
        AsnTable::compact_from_tsv(std::io::Cursor::new(TSV))
            .unwrap()
            .write(&dest)
            .unwrap();
        let before = std::fs::read(&dest).unwrap();

        let client = reqwest_middleware::ClientBuilder::new(reqwest::Client::new()).build();
        let err = refresh(&server.url("/garbage"), &dest, &client).await;

        assert!(matches!(err, Err(AsnError::Empty)));
        assert_eq!(
            std::fs::read(&dest).unwrap(),
            before,
            "a working table must survive a garbage response"
        );
    }
}

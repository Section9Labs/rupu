//! IP → ASN range table.
//!
//! Sorted ranges + binary search. v4 and v6 are held separately so the
//! v4 path compares `u32` rather than widening every address.

use serde::{Deserialize, Serialize};
use std::io::BufRead;
use std::net::IpAddr;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AsnInfo {
    pub asn: u32,
    pub org: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Range<T> {
    start: T,
    end: T,
    asn: u32,
    /// Index into `AsnTable::orgs`.
    org: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AsnTable {
    v4: Vec<Range<u32>>,
    v6: Vec<Range<u128>>,
    orgs: Vec<String>,
}

impl AsnTable {
    pub fn len(&self) -> usize {
        self.v4.len() + self.v6.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Build from iptoasn.com's `ip2asn-combined.tsv`.
    ///
    /// Columns: start, end, AS number, country, description. Rows with
    /// AS number 0 are unrouted and are skipped. Malformed rows are
    /// skipped rather than failing the whole ingest.
    pub fn compact_from_tsv<R: BufRead>(reader: R) -> std::io::Result<Self> {
        let mut table = AsnTable::default();
        let mut org_index: std::collections::HashMap<String, u32> =
            std::collections::HashMap::new();

        for line in reader.lines() {
            let line = line?;
            let mut cols = line.split('\t');
            let (Some(start), Some(end), Some(asn), Some(_cc), Some(desc)) = (
                cols.next(),
                cols.next(),
                cols.next(),
                cols.next(),
                cols.next(),
            ) else {
                continue;
            };
            let Ok(asn) = asn.parse::<u32>() else {
                continue;
            };
            if asn == 0 {
                continue;
            }
            let (Ok(start), Ok(end)) = (start.parse::<IpAddr>(), end.parse::<IpAddr>()) else {
                continue;
            };

            let next_idx = org_index.len() as u32;
            let org = *org_index.entry(desc.to_string()).or_insert(next_idx);
            if org as usize == table.orgs.len() {
                table.orgs.push(desc.to_string());
            }

            match (start, end) {
                (IpAddr::V4(s), IpAddr::V4(e)) => table.v4.push(Range {
                    start: s.into(),
                    end: e.into(),
                    asn,
                    org,
                }),
                (IpAddr::V6(s), IpAddr::V6(e)) => table.v6.push(Range {
                    start: s.into(),
                    end: e.into(),
                    asn,
                    org,
                }),
                _ => continue,
            }
        }

        table.v4.sort_unstable_by_key(|r| r.start);
        table.v6.sort_unstable_by_key(|r| r.start);
        Ok(table)
    }

    pub fn write(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("db.tmp");
        let json = serde_json::to_vec(self)?;
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, path)
    }

    pub fn load(path: &Path) -> std::io::Result<Self> {
        let bytes = std::fs::read(path)?;
        serde_json::from_slice(&bytes)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }

    pub fn lookup(&self, ip: IpAddr) -> Option<AsnInfo> {
        match ip {
            IpAddr::V4(a) => find(&self.v4, u32::from(a)).map(|r| self.info(r.asn, r.org)),
            IpAddr::V6(a) => find(&self.v6, u128::from(a)).map(|r| self.info(r.asn, r.org)),
        }
    }

    fn info(&self, asn: u32, org: u32) -> AsnInfo {
        AsnInfo {
            asn,
            org: self
                .orgs
                .get(org as usize)
                .cloned()
                .unwrap_or_else(|| format!("AS{asn}")),
        }
    }
}

/// Last range whose `start <= needle`, then an inclusive `end` check.
fn find<T: Ord + Copy>(ranges: &[Range<T>], needle: T) -> Option<&Range<T>> {
    let idx = ranges.partition_point(|r| r.start <= needle);
    let candidate = ranges.get(idx.checked_sub(1)?)?;
    (needle <= candidate.end).then_some(candidate)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    const TSV: &str = "\
1.0.0.0\t1.0.0.255\t13335\tUS\tCLOUDFLARENET
1.0.1.0\t1.0.3.255\t0\tNone\tNot routed
8.8.8.0\t8.8.8.255\t15169\tUS\tGOOGLE
2606:4700::\t2606:4700:ffff:ffff:ffff:ffff:ffff:ffff\t13335\tUS\tCLOUDFLARENET
";

    fn table() -> AsnTable {
        AsnTable::compact_from_tsv(Cursor::new(TSV)).unwrap()
    }

    #[test]
    fn looks_up_an_ipv4_inside_a_range() {
        let t = table();
        let got = t.lookup("1.0.0.42".parse().unwrap()).unwrap();
        assert_eq!(got.asn, 13335);
        assert_eq!(got.org, "CLOUDFLARENET");
    }

    #[test]
    fn range_boundaries_are_inclusive() {
        let t = table();
        assert!(t.lookup("1.0.0.0".parse().unwrap()).is_some());
        assert!(t.lookup("1.0.0.255".parse().unwrap()).is_some());
    }

    #[test]
    fn unrouted_ranges_are_not_indexed() {
        let t = table();
        assert!(t.lookup("1.0.2.1".parse().unwrap()).is_none());
    }

    #[test]
    fn unmapped_address_returns_none() {
        let t = table();
        assert!(t.lookup("192.0.2.1".parse().unwrap()).is_none());
    }

    #[test]
    fn looks_up_an_ipv6_address() {
        let t = table();
        let got = t.lookup("2606:4700::1111".parse().unwrap()).unwrap();
        assert_eq!(got.asn, 13335);
    }

    #[test]
    fn write_then_load_round_trips() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("asn.db");
        table().write(&path).unwrap();

        let loaded = AsnTable::load(&path).unwrap();
        assert_eq!(
            loaded.lookup("8.8.8.8".parse().unwrap()).unwrap().asn,
            15169
        );
        assert_eq!(loaded.len(), table().len());
    }

    #[test]
    fn load_of_a_missing_file_is_an_error_callers_can_degrade_on() {
        let tmp = tempfile::TempDir::new().unwrap();
        let err = AsnTable::load(&tmp.path().join("absent.db")).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    }

    #[test]
    fn malformed_tsv_rows_are_skipped_not_fatal() {
        let t = AsnTable::compact_from_tsv(Cursor::new(
            "garbage\n1.0.0.0\t1.0.0.255\t13335\tUS\tCLOUDFLARENET\n\n",
        ))
        .unwrap();
        assert_eq!(t.len(), 1);
    }
}

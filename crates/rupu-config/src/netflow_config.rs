//! `[netflow]` — automatic ASN-table acquisition/refresh for outbound-peer
//! enrichment. The operator should never have to run a command to get
//! enrichment working; Task 7 (ASN acquisition) and `cp serve`'s refresh
//! tick read these values.

use serde::{Deserialize, Serialize};

/// `[netflow]` config section: governs automatic acquisition/refresh of the
/// ASN table used to enrich rupu's own outbound-peer IPs with ASN/org data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NetflowConfig {
    /// Acquire and refresh the ASN table automatically. Defaults to `true`
    /// so enrichment works without an operator running a command.
    #[serde(default = "NetflowConfig::default_asn_auto_refresh")]
    pub asn_auto_refresh: bool,
    /// Refresh cadence, in days. BGP prefixes move slowly; weekly is ample.
    /// Defaults to 7.
    #[serde(default = "NetflowConfig::default_asn_refresh_interval_days")]
    pub asn_refresh_interval_days: u64,
    /// Source for the combined IPv4+IPv6 prefix→ASN table. Defaults to
    /// iptoasn.com's combined TSV.
    #[serde(default = "NetflowConfig::default_asn_source_url")]
    pub asn_source_url: String,
}

impl NetflowConfig {
    fn default_asn_auto_refresh() -> bool {
        true
    }

    fn default_asn_refresh_interval_days() -> u64 {
        7
    }

    fn default_asn_source_url() -> String {
        "https://iptoasn.com/data/ip2asn-combined.tsv.gz".to_string()
    }
}

impl Default for NetflowConfig {
    fn default() -> Self {
        Self {
            asn_auto_refresh: Self::default_asn_auto_refresh(),
            asn_refresh_interval_days: Self::default_asn_refresh_interval_days(),
            asn_source_url: Self::default_asn_source_url(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::NetflowConfig;

    #[test]
    fn netflow_defaults_are_auto_refresh_weekly() {
        let cfg = NetflowConfig::default();
        assert!(cfg.asn_auto_refresh);
        assert_eq!(cfg.asn_refresh_interval_days, 7);
        assert!(cfg.asn_source_url.contains("iptoasn.com"));
    }

    #[test]
    fn netflow_section_parses_from_toml() {
        let toml = r#"
[netflow]
asn_auto_refresh = false
asn_refresh_interval_days = 30
"#;
        let cfg: NetflowConfig = toml::from_str::<toml::Value>(toml)
            .unwrap()
            .get("netflow")
            .unwrap()
            .clone()
            .try_into()
            .unwrap();
        assert!(!cfg.asn_auto_refresh);
        assert_eq!(cfg.asn_refresh_interval_days, 30);
        // Unspecified keys still take their default.
        assert!(cfg.asn_source_url.contains("iptoasn.com"));
    }
}

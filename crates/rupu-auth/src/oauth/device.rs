//! GitHub device-code OAuth flow. Implementation lands in Task 7.

use crate::backend::ProviderId;
use crate::stored::StoredCredential;
use anyhow::Result;

pub async fn run(_provider: ProviderId) -> Result<StoredCredential> {
    anyhow::bail!("oauth_device::run not yet implemented")
}

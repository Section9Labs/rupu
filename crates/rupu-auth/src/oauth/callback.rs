//! PKCE browser-callback OAuth flow. Implementation lands in Task 6.

use crate::backend::ProviderId;
use crate::stored::StoredCredential;
use anyhow::Result;

pub async fn run(_provider: ProviderId) -> Result<StoredCredential> {
    anyhow::bail!("oauth_callback::run not yet implemented")
}

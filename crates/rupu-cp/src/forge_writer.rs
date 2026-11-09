//! Optional operator-only forge-write port; credentials stay in the owning adapter.
pub use rupu_scm::forge_write::{
    ForgeCapability, ForgeError, ForgeOwner, ForgeRequest, PreparedMutation, PreparedWrite,
};

#[async_trait::async_trait]
pub trait ForgeWriter: Send + Sync {
    fn capabilities(&self) -> Vec<ForgeCapability>;
    async fn prepare(
        &self,
        request: &ForgeRequest,
        owner: Option<&ForgeOwner>,
    ) -> Result<PreparedWrite, ForgeError>;
    async fn preflight(
        &self,
        intent: &PreparedWrite,
        owner: Option<&ForgeOwner>,
    ) -> Result<Box<dyn PreparedMutation>, ForgeError>;
}

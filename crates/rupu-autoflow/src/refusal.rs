/// A refusal by its kind: what the control plane answers 400 / 404 / 409
/// for. Raised inside an `anyhow` chain, so the CLI prints its message as
/// it prints any other error.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    /// An argument the operation can't take (a bad issue ref, a bad repo
    /// filter, conflicting flags, a mode autoflows don't run in).
    #[error("{0}")]
    Invalid(String),
    /// Nothing by that name (no visible autoflow, no claim).
    #[error("{0}")]
    NotFound(String),
    /// The state forbids it now (an ambiguous autoflow name, a claim
    /// another cycle holds).
    #[error("{0}")]
    Conflict(String),
}

/// The [`Refusal`] anywhere in `e`'s chain, if any.
pub fn refusal_of(e: &anyhow::Error) -> Option<&Refusal> {
    e.chain().find_map(|cause| cause.downcast_ref::<Refusal>())
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Context as _;

    #[test]
    fn a_refusal_is_found_under_context() {
        let e = Err::<(), _>(anyhow::Error::from(Refusal::NotFound("no claim".into())))
            .context("repair")
            .unwrap_err();
        assert_eq!(refusal_of(&e), Some(&Refusal::NotFound("no claim".into())));
        assert_eq!(refusal_of(&anyhow::anyhow!("disk on fire")), None);
    }
}

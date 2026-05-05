use rupu_orchestrator::Workflow;

/// Derive parent → child edges for the v0 spec shape: linear chain
/// of `steps`. Fan-out steps (`for_each:` / `parallel:`) produce
/// edges from the prior step to each parallel child by step_id; the
/// projection table treats parallel children as siblings of the
/// fan-out node (drawn as a vertical drop in canvas mode).
///
/// Returns edges in deterministic spec-declaration order so layout
/// is stable across runs.
pub fn derive_edges(wf: &Workflow) -> Vec<(String, String)> {
    let mut edges = Vec::new();
    let ids: Vec<&str> = wf.steps.iter().map(|s| s.id.as_str()).collect();
    for w in ids.windows(2) {
        edges.push((w[0].to_string(), w[1].to_string()));
    }
    edges
}

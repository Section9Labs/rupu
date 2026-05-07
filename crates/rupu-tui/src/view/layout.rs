use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Position {
    pub col: u16,
    pub row: u16,
}

pub fn layout_canvas(node_ids: &[&str], edges: &[(String, String)]) -> BTreeMap<String, Position> {
    let mut depth: BTreeMap<&str, u16> = BTreeMap::new();
    for id in node_ids {
        depth.insert(id, 0);
    }
    let mut changed = true;
    while changed {
        changed = false;
        for (parent, child) in edges {
            let parent_d = *depth.get(parent.as_str()).unwrap_or(&0);
            let child_d = *depth.get(child.as_str()).unwrap_or(&0);
            if child_d <= parent_d {
                depth.insert(child.as_str(), parent_d + 1);
                changed = true;
            }
        }
    }

    let mut by_col: BTreeMap<u16, Vec<&str>> = BTreeMap::new();
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for id in node_ids {
        if seen.insert(id) {
            by_col
                .entry(*depth.get(id).unwrap_or(&0))
                .or_default()
                .push(id);
        }
    }

    let mut out = BTreeMap::new();
    for (col, ids) in by_col {
        for (row, id) in ids.into_iter().enumerate() {
            #[allow(clippy::cast_possible_truncation)]
            // Upper bound: workflows with >65535 fan-out children is impossible
            out.insert(
                id.to_string(),
                Position {
                    col,
                    row: row as u16,
                },
            );
        }
    }
    out
}

/// Pre-order DFS yielding (step_id, indent_depth) for tree view.
pub fn layout_tree(node_ids: &[&str], edges: &[(String, String)]) -> Vec<(String, u16)> {
    let mut children: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    let mut has_parent: BTreeSet<&str> = BTreeSet::new();
    for (p, c) in edges {
        children.entry(p.as_str()).or_default().push(c.as_str());
        has_parent.insert(c.as_str());
    }
    let roots: Vec<&str> = node_ids
        .iter()
        .copied()
        .filter(|id| !has_parent.contains(id))
        .collect();

    let mut out = Vec::new();
    fn dfs<'a>(
        node: &'a str,
        depth: u16,
        children: &BTreeMap<&'a str, Vec<&'a str>>,
        out: &mut Vec<(String, u16)>,
    ) {
        out.push((node.to_string(), depth));
        if let Some(kids) = children.get(node) {
            for kid in kids {
                let next_depth = if kids.len() == 1 { depth } else { depth + 1 };
                dfs(kid, next_depth, children, out);
            }
        }
    }
    for r in roots {
        dfs(r, 0, &children, &mut out);
    }
    out
}

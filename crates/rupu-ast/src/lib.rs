//! rupu-ast — tree-sitter CST wrapper: parse source and serialize a
//! bounded, JSON-friendly subtree around a target position.
#![forbid(unsafe_code)]

use serde::Serialize;

pub const MAX_AST_NODES: usize = 2000;
pub const CONTEXT_ANCESTORS: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Rust,
    Python,
    TypeScript,
    Tsx,
    JavaScript,
    Go,
    Json,
}

impl Lang {
    pub fn from_path(p: &std::path::Path) -> Option<Lang> {
        match p.extension().and_then(|e| e.to_str())? {
            "rs" => Some(Lang::Rust),
            "py" => Some(Lang::Python),
            "ts" => Some(Lang::TypeScript),
            "tsx" => Some(Lang::Tsx),
            "js" | "jsx" | "mjs" | "cjs" => Some(Lang::JavaScript),
            "go" => Some(Lang::Go),
            "json" => Some(Lang::Json),
            _ => None,
        }
    }

    pub fn grammar(self) -> tree_sitter::Language {
        match self {
            Lang::Rust => tree_sitter_rust::LANGUAGE.into(),
            Lang::Python => tree_sitter_python::LANGUAGE.into(),
            Lang::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Lang::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
            Lang::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
            Lang::Go => tree_sitter_go::LANGUAGE.into(),
            Lang::Json => tree_sitter_json::LANGUAGE.into(),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AstNode {
    pub kind: String,
    pub named: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    pub start_line: u32,
    pub start_col: u32,
    pub end_line: u32,
    pub end_col: u32,
    pub matched: bool,
    pub children: Vec<AstNode>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AstSubtree {
    pub language: String, // lowercase name
    pub root: AstNode,
    pub truncated: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum AstError {
    #[error("failed to set tree-sitter language")]
    Language,
    #[error("tree-sitter produced no tree")]
    NoTree,
}

/// Parse `source`, find the deepest NAMED node containing the 1-based
/// (line,col), root the returned subtree CONTEXT_ANCESTORS named
/// ancestors above it, serialize (1-based, matched node flagged),
/// capped at MAX_AST_NODES nodes.
pub fn parse_slice(source: &str, lang: Lang, line: u32, col: u32) -> Result<AstSubtree, AstError> {
    parse_slice_capped(source, lang, line, col, MAX_AST_NODES)
}

/// Same as [`parse_slice`] but with an explicit node cap (for testing the
/// truncation path with a small budget).
fn parse_slice_capped(
    source: &str,
    lang: Lang,
    line: u32,
    col: u32,
    cap: usize,
) -> Result<AstSubtree, AstError> {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&lang.grammar())
        .map_err(|_| AstError::Language)?;
    let tree = parser.parse(source, None).ok_or(AstError::NoTree)?;
    let root = tree.root_node();

    // tree-sitter Point is 0-based (row, column-in-bytes). Convert the
    // 1-based (line,col) target to a 0-based point. `col` is passed straight
    // through as `Point.column` (a BYTE offset), which is correct: `col`
    // originates from ast-grep, which likewise reports byte-based tree-sitter
    // columns — so no char/byte conversion is needed. Do NOT "fix" this to a
    // char index.
    let target = tree_sitter::Point {
        row: line.saturating_sub(1) as usize,
        column: col.saturating_sub(1) as usize,
    };

    // Descend to the smallest named node whose range contains `target`.
    let matched = deepest_named_at(root, target).unwrap_or(root);
    let matched_id = matched.id();

    // Walk up CONTEXT_ANCESTORS named ancestors for context.
    let mut ctx = matched;
    for _ in 0..CONTEXT_ANCESTORS {
        match ctx.parent() {
            Some(p) => ctx = p,
            None => break,
        }
    }

    let mut budget = cap;
    let mut truncated = false;
    let node = serialize(ctx, None, matched_id, &mut budget, &mut truncated)
        .expect("budget starts > 0 so the root node always serializes");
    Ok(AstSubtree {
        language: lang_name(lang).to_string(),
        root: node,
        truncated,
    })
}

fn deepest_named_at(node: tree_sitter::Node, pt: tree_sitter::Point) -> Option<tree_sitter::Node> {
    // named_descendant_for_point_range is the direct tree-sitter API for this.
    node.named_descendant_for_point_range(pt, pt)
}

fn serialize(
    node: tree_sitter::Node,
    field: Option<String>,
    matched_id: usize,
    budget: &mut usize,
    truncated: &mut bool,
) -> Option<AstNode> {
    // Count THIS node against the budget at entry. If nothing is left, this
    // node can't be emitted — flag truncation and drop it (and its subtree).
    if *budget == 0 {
        *truncated = true;
        return None;
    }
    *budget -= 1;

    let start = node.start_position();
    let end = node.end_position();
    let mut children = Vec::new();
    let mut cursor = node.walk();
    for (i, child) in node.children(&mut cursor).enumerate() {
        let fname = node.field_name_for_child(i as u32).map(|s| s.to_string());
        match serialize(child, fname, matched_id, budget, truncated) {
            Some(c) => children.push(c),
            None => break, // budget exhausted; `truncated` already set
        }
    }
    Some(AstNode {
        kind: node.kind().to_string(),
        named: node.is_named(),
        field,
        start_line: start.row as u32 + 1,
        start_col: start.column as u32 + 1,
        end_line: end.row as u32 + 1,
        end_col: end.column as u32 + 1,
        matched: node.id() == matched_id,
        children,
    })
}

fn lang_name(l: Lang) -> &'static str {
    match l {
        Lang::Rust => "rust",
        Lang::Python => "python",
        Lang::TypeScript => "typescript",
        Lang::Tsx => "tsx",
        Lang::JavaScript => "javascript",
        Lang::Go => "go",
        Lang::Json => "json",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rust_and_marks_matched_node() {
        let src = "fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n";
        // target the identifier `add` at line 1, col 4 (1-based)
        let sub = parse_slice(src, Lang::Rust, 1, 4).expect("parse");
        assert!(!sub.truncated);
        // somewhere in the tree there is exactly the matched node, named, kind identifier
        let matched = find_matched(&sub.root).expect("a matched node");
        assert!(matched.named);
        assert_eq!(matched.kind, "identifier");
        assert_eq!(matched.start_line, 1);
        // root is a named ancestor (context), not the identifier itself
        assert_ne!(sub.root.kind, "identifier");
    }

    #[test]
    fn lang_from_path_maps_extensions() {
        assert_eq!(
            Lang::from_path(std::path::Path::new("a.rs")),
            Some(Lang::Rust)
        );
        assert_eq!(
            Lang::from_path(std::path::Path::new("a.tsx")),
            Some(Lang::Tsx)
        );
        assert_eq!(Lang::from_path(std::path::Path::new("a.unknown")), None);
    }

    #[test]
    fn every_language_parses_a_trivial_snippet() {
        for (lang, src) in [
            (Lang::Rust, "fn a(){}"),
            (Lang::Python, "def a():\n    pass\n"),
            (Lang::TypeScript, "const a: number = 1;"),
            (Lang::Tsx, "const a = <div/>;"),
            (Lang::JavaScript, "const a = 1;"),
            (Lang::Go, "package main\nfunc a(){}"),
            (Lang::Json, "{\"a\":1}"),
        ] {
            assert!(
                parse_slice(src, lang, 1, 1).is_ok(),
                "{lang:?} failed to parse"
            );
        }
    }

    #[test]
    fn cap_truncates_and_flags() {
        let src = "fn f() { let a = (1 + (2 + (3 + 4))); let b = 5; let c = 6; }";
        let sub = parse_slice_capped(src, Lang::Rust, 1, 4, 5).unwrap();
        assert!(sub.truncated, "small cap should truncate");
        // total serialized nodes <= cap
        fn count(n: &AstNode) -> usize {
            1 + n.children.iter().map(count).sum::<usize>()
        }
        assert!(count(&sub.root) <= 5);
    }

    // test helper
    fn find_matched(n: &AstNode) -> Option<&AstNode> {
        if n.matched {
            return Some(n);
        }
        n.children.iter().find_map(find_matched)
    }
}

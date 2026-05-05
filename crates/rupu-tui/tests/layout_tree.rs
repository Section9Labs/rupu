use rupu_tui::view::layout::layout_tree;

#[test]
fn linear_chain_emits_in_order_with_zero_indent() {
    let edges = vec![
        ("a".into(), "b".into()),
        ("b".into(), "c".into()),
    ];
    let lines = layout_tree(&["a", "b", "c"], &edges);
    assert_eq!(lines, vec![
        ("a".to_string(), 0),
        ("b".to_string(), 0),
        ("c".to_string(), 0),
    ]);
}

#[test]
fn fanout_indents_children_to_depth_one() {
    let edges = vec![
        ("a".into(), "b".into()),
        ("a".into(), "c".into()),
    ];
    let lines = layout_tree(&["a", "b", "c"], &edges);
    assert_eq!(lines[0], ("a".to_string(), 0));
    assert_eq!(lines[1].1, 1);
    assert_eq!(lines[2].1, 1);
}

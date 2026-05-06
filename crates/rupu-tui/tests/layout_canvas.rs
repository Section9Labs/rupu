use rupu_tui::view::layout::{layout_canvas, Position};

#[test]
fn linear_chain_is_one_row_three_cols() {
    let edges = vec![
        ("a".into(), "b".into()),
        ("b".into(), "c".into()),
    ];
    let positions = layout_canvas(&["a", "b", "c"], &edges);
    assert_eq!(positions.get("a"), Some(&Position { col: 0, row: 0 }));
    assert_eq!(positions.get("b"), Some(&Position { col: 1, row: 0 }));
    assert_eq!(positions.get("c"), Some(&Position { col: 2, row: 0 }));
}

#[test]
fn fanout_packs_children_in_next_column() {
    let edges = vec![
        ("a".into(), "b".into()),
        ("a".into(), "c".into()),
    ];
    let positions = layout_canvas(&["a", "b", "c"], &edges);
    assert_eq!(positions["a"], Position { col: 0, row: 0 });
    assert_eq!(positions["b"].col, 1);
    assert_eq!(positions["c"].col, 1);
    assert_ne!(positions["b"].row, positions["c"].row);
}

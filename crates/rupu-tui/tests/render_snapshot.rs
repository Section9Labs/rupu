use ratatui::{backend::TestBackend, buffer::Buffer, Terminal};
use rupu_tui::state::{NodeStatus, RunModel};
use rupu_tui::view::canvas::render_canvas;
use rupu_tui::view::panel::render_panel;
use rupu_tui::view::tree::render_tree;

fn buffer_to_string(b: &Buffer) -> String {
    let mut s = String::new();
    for y in 0..b.area.height {
        for x in 0..b.area.width {
            s.push(b[(x, y)].symbol().chars().next().unwrap_or(' '));
        }
        s.push('\n');
    }
    s
}

#[test]
fn linear_three_node_canvas() {
    let mut model = RunModel::new();
    model.upsert_node("a", "spec-agent").status = NodeStatus::Complete;
    model.upsert_node("b", "planner").status = NodeStatus::Complete;
    model.upsert_node("c", "code-agent").status = NodeStatus::Working;

    let edges = vec![("a".into(), "b".into()), ("b".into(), "c".into())];
    let backend = TestBackend::new(80, 12);
    let mut term = Terminal::new(backend).unwrap();
    term.draw(|f| render_canvas(f, f.area(), &model, &edges, "c")).unwrap();

    let buf = term.backend().buffer().clone();
    insta::assert_snapshot!("canvas_linear", buffer_to_string(&buf));
}

#[test]
fn fanout_tree_render() {
    let mut model = RunModel::new();
    model.upsert_node("a", "spec").status = NodeStatus::Complete;
    model.upsert_node("b", "test").status = NodeStatus::Waiting;
    model.upsert_node("c", "sec").status = NodeStatus::Waiting;

    let edges = vec![("a".into(), "b".into()), ("a".into(), "c".into())];
    let backend = TestBackend::new(40, 8);
    let mut term = Terminal::new(backend).unwrap();
    term.draw(|f| render_tree(f, f.area(), &model, &edges, "a")).unwrap();

    insta::assert_snapshot!("tree_fanout", buffer_to_string(&term.backend().buffer().clone()));
}

#[test]
fn panel_shows_status_tools_tokens() {
    let mut model = RunModel::new();
    let n = model.upsert_node("code-agent", "claude-sonnet-4-6");
    n.status = NodeStatus::Working;
    n.tokens.input = 1902;
    n.tokens.output = 311;
    n.tools_used.insert("bash".into(), 3);
    n.tools_used.insert("read".into(), 8);
    n.tools_used.insert("edit".into(), 1);

    let backend = TestBackend::new(40, 12);
    let mut term = Terminal::new(backend).unwrap();
    term.draw(|f| render_panel(f, f.area(), &model, "code-agent")).unwrap();

    insta::assert_snapshot!("panel_focused", buffer_to_string(&term.backend().buffer().clone()));
}

#[test]
fn too_narrow_terminal_renders_warning() {
    use rupu_tui::view::canvas::render_canvas_with_warning;
    let mut model = RunModel::new();
    model.upsert_node("a", "x").status = NodeStatus::Working;
    let edges = vec![];
    let backend = TestBackend::new(38, 4);
    let mut term = Terminal::new(backend).unwrap();
    term.draw(|f| render_canvas_with_warning(f, f.area(), &model, &edges, "a")).unwrap();
    let s = buffer_to_string(&term.backend().buffer().clone());
    assert!(s.contains("canvas truncated"), "got:\n{s}");
}

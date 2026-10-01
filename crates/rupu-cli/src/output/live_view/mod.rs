//! The `workflow run` live view: a three-pane dashboard (specs 2026-09-30 and
//! 2026-10-01). The pure model and layout — [`row`] (styled `Line`s), [`nav`]
//! (selection / drill / pane-focus state), [`layout`] (the header block and
//! the key-legend footer), [`gate`] (the gate-details panel), [`structure`]
//! (the workflow DAG as live-overlaid rails, the dashboard's structure pane)
//! and [`panes`] (`dashboard_frame`: the whole frame, composing the header,
//! the structure pane, the stream, the firehose and the footer) — and the two
//! modules that do I/O: [`mux`] (the bounded transcript firehose: it tails
//! transcript files) and [`render`] (the alt-screen guard and diff renderer:
//! it writes the terminal). `output::live_run` drives them.
pub mod gate;
pub mod layout;
pub mod mux;
pub mod nav;
pub mod panes;
pub mod render;
pub mod row;
pub mod structure;

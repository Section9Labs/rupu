//! The redesigned `workflow run` live view (spec 2026-09-30). The pure model
//! and layout — [`row`] (styled `Line`s), [`nav`] (selection / drill state),
//! [`layout`] (the whole-frame composition) and [`gate`] (the gate-details
//! panel), plus [`structure`] (the workflow DAG as live-overlaid rails, the
//! dashboard's structure pane) — and the two modules that do I/O: [`mux`] (the bounded transcript
//! firehose: it tails transcript files) and [`render`] (the alt-screen guard
//! and diff renderer: it writes the terminal). `output::live_run` drives them.
pub mod gate;
pub mod layout;
pub mod mux;
pub mod nav;
pub mod render;
pub mod row;
pub mod structure;

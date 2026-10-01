//! Pure interaction + layout model for the redesigned `workflow run` live
//! view (spec 2026-09-30). No terminal I/O — Plan 3 renders these rows and
//! feeds events in.
pub mod gate;
pub mod layout;
pub mod mux;
pub mod nav;
pub mod render;
pub mod row;

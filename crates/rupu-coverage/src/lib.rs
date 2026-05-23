//! rupu coverage harness — exhaustive-coverage ledgers, concern catalogs, and agent tools.

#![deny(clippy::all)]
#![forbid(unsafe_code)]

#[cfg(test)]
mod tests {
    #[test]
    fn crate_compiles() {
        // Sentinel: this test exists only so `cargo test -p rupu-coverage`
        // exercises the crate skeleton; later tasks replace it.
        assert_eq!(2 + 2, 4);
    }
}

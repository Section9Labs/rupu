//! Keep `rupu-cp` self-building even when the web frontend hasn't been built.
//!
//! `rust-embed`'s `#[folder = "web/dist/"]` macro requires the directory to
//! exist at compile time. When `web/dist/index.html` is missing (fresh clone,
//! CI without a Node toolchain), we write an honest placeholder so `cargo
//! build`/`cargo test` succeed standalone. A real `npm run build` overwrites it.

use std::fs;
use std::path::Path;

const PLACEHOLDER: &str = "<!doctype html>\n<html lang=\"en\">\n  <head>\n    <meta charset=\"UTF-8\" />\n    <title>rupu Control Plane</title>\n  </head>\n  <body>\n    <h1>rupu Control Plane</h1>\n    <p>Web UI not built. Run <code>npm run build</code> in crates/rupu-cp/web.</p>\n  </body>\n</html>\n";

fn main() {
    let dist = Path::new("web/dist");
    let index = dist.join("index.html");

    if !index.exists() {
        fs::create_dir_all(dist).expect("failed to create web/dist");
        fs::write(&index, PLACEHOLDER).expect("failed to write placeholder index.html");
        println!(
            "cargo:warning=rupu-cp: web/dist not found; embedding placeholder. Run `npm run build` in crates/rupu-cp/web for the real UI."
        );
    }

    // A real `npm run build` updating web/dist triggers a re-embed.
    println!("cargo:rerun-if-changed=web/dist");
}

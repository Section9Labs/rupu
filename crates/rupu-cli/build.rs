fn main() {
    println!("cargo:rerun-if-env-changed=RUPU_RELEASE_CHANNEL");
    println!("cargo:rerun-if-env-changed=RUPU_RELEASE_VERSION");
}

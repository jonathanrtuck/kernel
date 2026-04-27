use std::env;
use std::path::PathBuf;

fn main() {
    let target = env::var("TARGET").unwrap_or_default();
    if target == "aarch64-unknown-none" {
        let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
        let link_ld = manifest_dir.join("link.ld");
        println!("cargo:rustc-link-arg=-T{}", link_ld.display());
        println!("cargo:rerun-if-changed=link.ld");
    }
}

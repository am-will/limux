use std::{env, path::PathBuf};

fn main() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let ghostty_root = manifest_dir.join("../../ghostty");
    let glad_src = ghostty_root.join("vendor/glad/src/gl.c");
    let glad_include = ghostty_root.join("vendor/glad/include");

    cc::Build::new()
        .file(&glad_src)
        .include(&glad_include)
        .cargo_metadata(false)
        .compile("glad");

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR must be set"));
    let glad_archive = out_dir.join("libglad.a");
    println!("cargo:rustc-link-arg-bin=limux={}", glad_archive.display());

    println!("cargo:rerun-if-changed={}", glad_src.display());
    println!("cargo:rerun-if-changed={}", glad_include.display());
}

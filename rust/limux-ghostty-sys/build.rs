use std::path::PathBuf;

fn main() {
    // Find libghostty relative to the workspace root.
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let ghostty_root = manifest_dir.join("../../ghostty");
    let ghostty_lib = ghostty_root
        .join("zig-out/lib")
        .canonicalize()
        .expect("libghostty not found — run: cd ghostty && zig build -Dapp-runtime=none -Doptimize=ReleaseFast");

    println!("cargo:rustc-link-search=native={}", ghostty_lib.display());
    println!("cargo:rustc-link-lib=dylib=ghostty");
    println!("cargo:rustc-link-lib=dylib=epoxy");

    // Compile glad (GL loader) which libghostty depends on but doesn't
    // include when built as a shared library. Force the archive in because
    // libghostty is linked after Rust rlibs and otherwise these objects may be
    // skipped before libghostty's undefined gladLoader* symbols are seen.
    let glad_src = ghostty_root.join("vendor/glad/src/gl.c");
    let glad_include = ghostty_root.join("vendor/glad/include");
    if glad_src.exists() {
        cc::Build::new()
            .cargo_metadata(false)
            .file(&glad_src)
            .include(&glad_include)
            .compile("glad");
        let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR is set by Cargo"));
        println!("cargo:rustc-link-search=native={}", out_dir.display());
        println!("cargo:rustc-link-lib=static:+whole-archive=glad");
    }

    // Re-run if libghostty changes
    println!(
        "cargo:rerun-if-changed={}",
        ghostty_lib.join("libghostty.so").display()
    );
}

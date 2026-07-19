use std::path::PathBuf;

fn main() {
    // Allow override via GHOSTTY_LIB_DIR env var (used by nix build).
    // Falls back to ../../ghostty/zig-out/lib for local dev builds.
    let ghostty_lib: PathBuf = if let Ok(dir) = std::env::var("GHOSTTY_LIB_DIR") {
        PathBuf::from(dir)
    } else {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let ghostty_root = manifest_dir.join("../../ghostty");
        ghostty_root
            .join("zig-out/lib")
            .canonicalize()
            .expect("libghostty not found — run: cd ghostty && zig build -Dapp-runtime=none -Doptimize=ReleaseFast")
    };

    println!("cargo:rustc-link-search=native={}", ghostty_lib.display());
    println!("cargo:rustc-link-lib=dylib=ghostty");
    println!("cargo:rustc-link-lib=dylib=epoxy");

    // Compile glad (GL loader) — may also need override path for nix build
    let glad_dir: PathBuf = if let Ok(dir) = std::env::var("GHOSTTY_SRC_DIR") {
        PathBuf::from(dir)
    } else {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        manifest_dir.join("../../ghostty")
    };

    let glad_src = glad_dir.join("vendor/glad/src/gl.c");
    let glad_include = glad_dir.join("vendor/glad/include");
    if glad_src.exists() {
        cc::Build::new()
            .file(&glad_src)
            .include(&glad_include)
            .compile("glad");
    }

    println!("cargo:rerun-if-env-changed=GHOSTTY_LIB_DIR");
    println!("cargo:rerun-if-env-changed=GHOSTTY_SRC_DIR");
}

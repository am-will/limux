use std::path::PathBuf;

fn main() {
    // Find libghostty relative to the workspace root.
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let ghostty_root = manifest_dir.join("../../ghostty");
    let ghostty_library = "libghostty-internal.so";
    let ghostty_lib = ghostty_root
        .join("zig-out/lib")
        .canonicalize()
        .expect("Ghostty library directory not found — run: cd ghostty && zig build -Dapp-runtime=none -Doptimize=ReleaseFast");

    if !ghostty_lib.join(ghostty_library).is_file() {
        panic!(
            "{ghostty_library} not found at {} — run: cd ghostty && zig build -Dapp-runtime=none -Doptimize=ReleaseFast",
            ghostty_lib.display()
        );
    }

    println!("cargo:rustc-link-search=native={}", ghostty_lib.display());
    println!("cargo:rustc-link-lib=dylib=ghostty-internal");
    println!("cargo:rustc-link-lib=dylib=epoxy");

    // Re-run if libghostty changes.
    println!(
        "cargo:rerun-if-changed={}",
        ghostty_lib.join(ghostty_library).display()
    );
}

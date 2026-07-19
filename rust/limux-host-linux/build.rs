use std::path::PathBuf;

fn main() {
    // Compile glad (GL loader) which libghostty.so depends on at link time.
    // This must be done from a bin crate's build.rs because we need
    // rustc-link-arg (which doesn't propagate from library build scripts).
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let ghostty_root = manifest_dir.join("../../ghostty");
    let glad_src = ghostty_root.join("vendor/glad/src/gl.c");
    let glad_include = ghostty_root.join("vendor/glad/include");
    if glad_src.exists() {
        let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
        let cc = cc::Build::new();
        let compiler = cc.get_compiler();
        let glad_o = out_dir.join("glad.o");
        let mut cmd = std::process::Command::new(compiler.path());
        cmd.args(compiler.args());
        cmd.arg("-c")
            .arg(glad_src.to_str().unwrap())
            .arg("-I")
            .arg(glad_include.to_str().unwrap())
            .arg("-O2")
            .arg("-o")
            .arg(glad_o.to_str().unwrap());
        let status = cmd.status().expect("failed to compile glad.c");
        assert!(status.success(), "glad.c compilation failed");

        // Pass the .o directly to the linker (after -lghostty),
        // resolving the glad symbols libghostty.so needs.
        println!("cargo:rustc-link-arg={}", glad_o.display());
    }

    println!("cargo:rerun-if-changed={}", glad_src.display());
}

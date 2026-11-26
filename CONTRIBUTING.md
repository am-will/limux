# Contributing

## Quality gate

Run this before you commit:

```bash
./scripts/check.sh
```

It checks formatting, clippy, and tests. [docs/maintainability.md](docs/maintainability.md) has the rules and the live GTK tests. [docs/releasing.md](docs/releasing.md) has the release procedure.

## Ghostty library

`./scripts/build-ghostty.sh` records the clean Ghostty commit, the library checksum, and the resolved library path next to the artifact. The quality gate and the GTK smoke tests reject missing or stale provenance before they start Cargo or a display session.

- Rebuild with `./scripts/build-ghostty.sh` after you update the submodule. Linking an installed library into `ghostty/zig-out/lib` does not establish build provenance.
- Set `ZIG=/path/to/zig` to select Zig 0.16.0.
- Set `LIMUX_BUILD_JOBS` to change the default of four build jobs.

## Rust formatting

Run rustfmt before you submit Rust changes:

```bash
cargo fmt --all
```

To check formatting without compiling or GPU tooling:

```bash
cargo fmt --all -- --check
```

The quality gate also checks formatting.

# Contributing

## Rust formatting

Run rustfmt before submitting Rust changes:

```bash
cargo fmt --all
```

To validate formatting without compiling or requiring GPU tooling, run:

```bash
cargo fmt --all -- --check
```

The repository quality gate also checks formatting as part of `./scripts/check.sh`.

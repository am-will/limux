# Maintainability Rules

This repository is optimized for long-term maintainability over short-term convenience.

## Quality Bar

Every change should keep the workspace green under the canonical local check:

```bash
./scripts/check.sh
```

That script is the source of truth for the repository quality gate and currently runs:

- `cargo fmt --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`

## Ground Rules

- Keep one source of truth for command metadata, flags, and business rules.
- Prefer small domain modules over monolithic files.
- Avoid duplicate behavior paths. Extend the primary implementation instead.
- Do not commit generated artifacts, build outputs, or cache files.
- Remove dead files and legacy paths when a canonical replacement exists.
- Add regression tests when fixing behavior or moving high-risk logic.

## Refactor Guidance

- Split by domain, not by vague helper names.
- Keep pure logic separate from GTK widget wiring where possible.
- Move test modules out of large production files when they obscure the main codepath.
- Treat clippy findings as maintainability work, not optional cleanup.

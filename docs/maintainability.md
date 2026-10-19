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

## Live GTK Regression Tests

For changes to terminal rendering, surface lifetime, or the live control bridge,
also run the maintained headless Weston harness from the repository root:

```bash
LIMUX_SMOKE_PROFILE=debug ./scripts/xvfb-smoke-test.sh
```

It requires Weston, Xvfb (`xvfb-run`), `xauth`, `jq`, `setsid`, `findmnt`,
`dbus-run-session`, the host build dependencies,
and the embedded Ghostty library built as described in the README. It builds the CLI and host,
uses a private socket and temporary session/configuration directories, and
retains logs on failure. The `Rust Quality` workflow runs this harness as well.

Tests that require a graphical display are marked `#[ignore]` so the ordinary
`cargo test --workspace` run can work without a display. They must be explicitly
invoked by the harness; an ignored test alone does not provide coverage in the
quality gate. Most run under Weston. The inline rename focus regression runs
under isolated Xvfb because headless Weston has no active input seat and cannot
validate the active-window focus contract.

The terminal shutdown regression checks that freeing a terminal selects its
own OpenGL context even when another widget's context is current. The harness
then exercises ten multi-pane workspace cycles: it waits for the target shell
to execute a readiness command, sends Ctrl+D at an empty prompt, checks that
only that terminal disappears and the other two remain healthy, and closes
the workspace to check that its child processes are released.

### Hardware driver validation

The default harness uses software rendering. To exercise a specific GPU without
opening windows on the desktop, opt into a private headless Weston GL compositor:

```bash
LIMUX_SMOKE_PROFILE=debug LIMUX_SMOKE_GRAPHICS=hardware \
  LIMUX_EXPECT_GL_VENDOR=NVIDIA LIMUX_EXPECT_GL_RENDERER='RTX 5070 Ti' \
  LIMUX_SMOKE_CYCLES=50 ./scripts/xvfb-smoke-test.sh
```

Hardware mode requires vendor and renderer substrings, matched without regard to
case against `glGetString` from the regression's current terminal context. A
different renderer fails the run instead of silently accepting a software
fallback. The test prints the actual GL vendor, renderer, and version. Hardware
runs retain these results and host/compositor logs in the printed temporary
directory, including on success. Use `LIMUX_SMOKE_KEEP_ARTIFACTS=1` to retain
successful software runs too. `LIMUX_SMOKE_CYCLES` accepts 1 through 100; the
default is ten.

Both modes use a private D-Bus session, control socket, and XDG directories,
plus a clean POSIX shell fixture and the locally built Ghostty resources.
The private bus does not activate desktop services such as portals or keyrings.
Cleanup retains the directory if private portal mounts have not disappeared.
Hardware mode removes the software-renderer overrides and requires Weston with
headless GL support. Do not count a missing GPU or unsupported compositor as a
passing hardware check. The hardware renderer evidence covers Limux's
Wayland/EGL path on the recorded GPU/driver, not X11, a native desktop compositor,
or another driver's version.
Keep software CI and run the hardware check on trusted code before releases
that change rendering or terminal lifetime. Do not run untrusted pull requests
automatically on a personal GPU workstation.

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

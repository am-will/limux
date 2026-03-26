# Plan: Ubuntu 22.04 Full-Parity Support Without Regressions

**Generated**: 2026-03-23

## Overview
Support Ubuntu 22.04 users without degrading newer distros by treating Jammy support as a compatibility project, not a broad dependency rollback. The clean path is:

1. Define the exact support contract for Jammy full parity.
2. Reproduce the real failures on Jammy with the current tree.
3. Keep the existing product surface intact unless a failing baseline proves a narrower change is required.
4. Prefer the smallest compatibility boundary that preserves current dependencies, browser support, shortcuts, packaging, and behavior on newer systems.

This plan assumes the requested target is **full feature parity on Ubuntu 22.04**, including the built-in browser, not terminal-only support.

## Prerequisites
- Ubuntu 22.04 test environment with `jammy-updates` enabled.
- A current Linux distro test environment matching the existing happy path.
- Ghostty submodule build available for `libghostty.so`.
- Ability to run GTK apps under a display server or `xvfb-run` where applicable.
- Context7 lookup completed for `gtk4-rs` feature gating semantics.
- Verified package availability for Jammy before changing dependency policy.

## Dependency Graph

```text
T1 ── T2 ── T3 ── T4 ──┬── T5 ──┐
                        │        │
                        └── T6* ─┼── T7 ── T8
                                 │
                                 ┘

* T6 is conditional. It only runs if T2/T3 prove Jammy needs terminal OpenGL compatibility work.
```

## Tasks

### T1: Define the Jammy Support Contract
- **depends_on**: []
- **location**: new `docs/internal/ubuntu-22.04-acceptance.md`, `docs/maintainability.md`
- **description**: Write an internal acceptance contract for Ubuntu 22.04 full parity before touching code. Define supported package channels, browser expectations, Ghostty requirements, display-stack assumptions, release artifact expectations, and the invariant that newer distros must retain the same feature surface and shortcuts. Keep this internal until the implementation is proven.
- **validation**: An internal acceptance checklist exists that defines success on Jammy and on newer distros, including browser, terminal rendering, packaging, and shortcut/doc invariants; maintainers can tell whether a proposed change weakens the product contract.
- **status**: Not Completed
- **log**:
- **files edited/created**:

### T2: Build a Reproducible Ubuntu 22.04 Baseline Harness
- **depends_on**: [T1]
- **location**: new `scripts/ci/jammy-smoke.sh`, optional new `docker/ubuntu-22.04/` or CI workflow files, `README.md`
- **description**: Create a deterministic Jammy reproduction path that installs the exact packages, builds Ghostty resources, builds Limux, and runs smoke checks. It must fail fast on missing runtime prerequisites before treating any failure as a Jammy compatibility issue. Capture separate evidence for: `apt-cache policy`, `pkg-config --modversion`, Rust dependency resolution, GTK/WebKit package installability, browser compilation, `ldd` on built artifacts, Ghostty resource discovery, terminal launch, and OpenGL context behavior.
- **validation**: One command or CI job reproduces current Jammy behavior and stores logs for build/package/runtime failures without manual guesswork; runtime prerequisite failures are clearly separated from Jammy-specific compatibility failures.
- **status**: Not Completed
- **log**:
- **files edited/created**:

### T3: Audit Current GTK/WebKit/Ghostty Usage Against Jammy Constraints
- **depends_on**: [T2]
- **location**: `rust/limux-host-linux/Cargo.toml`, `rust/limux-host-linux/src/main.rs`, `rust/limux-host-linux/src/pane.rs`, `rust/limux-host-linux/src/terminal.rs`, `scripts/package.sh`, `README.md`
- **description**: Inventory the full compatibility surface, not just `gtk4`. Produce a cause table covering `gtk4`, `gdk4-wayland`, `libadwaita`, `webkit6`, their corresponding system packages and pkg-config modules, required GTK/WebKit ABI levels, Jammy package/channel availability, Ghostty runtime loader/resource assumptions, and whether the existing `GtkGLArea` path fails due to version gating, packaging, or runtime OpenGL behavior. The key outcome is deciding whether the current crate family can stay with lower feature gates, or whether a family-wide change is genuinely unavoidable.
- **validation**: A written cause table maps each Jammy failure to one concrete cause across the full crate family and runtime prerequisites, and identifies the smallest safe fix; no speculative rollback is left in the plan.
- **status**: Not Completed
- **log**:
- **files edited/created**:

### T4: Fix Dependency Policy Without Breaking the Existing Feature Contract
- **depends_on**: [T1, T2, T3]
- **location**: `rust/limux-host-linux/Cargo.toml`, workspace `Cargo.lock`, `README.md`, packaging docs/scripts
- **description**: Update dependency declarations only after the audit proves what Jammy needs. The default path should remain full-featured, and optional features must stay real and buildable. Prefer lowering GTK feature gates or adjusting package expectations before considering a full gtk-rs family downgrade. Do not introduce empty/no-op features, fake compatibility flags, or hidden product regressions.
- **validation**: Default build remains browser-capable; `--features webkit` remains a real compile path if the feature still exists; newer-distro builds do not lose functionality; dependency changes are justified by the audit note.
- **status**: Not Completed
- **log**:
- **files edited/created**:

### T5: Restore or Preserve Browser Full Parity on Jammy
- **depends_on**: [T4]
- **location**: `rust/limux-host-linux/Cargo.toml`, `rust/limux-host-linux/src/pane.rs`, `README.md`, `scripts/package.sh`, install/package metadata
- **description**: Make Jammy browser support explicit and correct. If Jammy requires specific `jammy-updates` packages or packaging metadata, encode that in docs and packaging checks rather than removing WebKit from the product. If a browser feature split is needed for packaging, it must still preserve the default full-parity release path and keep feature flags honest.
- **validation**: Jammy can compile and launch the browser path with documented packages; README/install docs and packaging inputs match the actual dependency model.
- **status**: Not Completed
- **log**:
- **files edited/created**:

### T6: Isolate Any Required OpenGL Compatibility Work Behind a Dedicated Boundary
- **depends_on**: [T2, T3, T4]
- **location**: `rust/limux-host-linux/src/terminal.rs`, possible new `rust/limux-host-linux/src/gl_backend.rs`, tests near terminal integration
- **description**: Only if the baseline proves current `GtkGLArea` behavior is insufficient on Jammy, implement a narrowly scoped compatibility layer for OpenGL context handling. Keep it isolated from unrelated UI behavior. Do not mix in keybinding changes, telemetry/state files, or unrelated UX changes. Prefer a dedicated module with explicit runtime selection and logging over a large inline rewrite inside `terminal.rs`. If T3 proves no GL compatibility work is needed, record a concrete skip artifact and keep the existing renderer path.
- **validation**: If T6 runs, Jammy terminal rendering succeeds and newer distros preserve existing behavior; if T6 is skipped, T3/T4 contain explicit evidence that the existing renderer path is sufficient. In both cases, no unrelated product-surface changes are bundled.
- **status**: Not Completed
- **log**:
- **files edited/created**:

### T7: Add Regression Gates for Feature Integrity and Cross-Distro Safety
- **depends_on**: [T4, T5, T6]
- **location**: `scripts/check.sh`, CI workflow files, tests under `rust/limux-host-linux`, new smoke/build scripts
- **description**: Add automated gates that prevent a repeat of the problematic PR shape. Cover exact matrix commands and artifact validation: `./scripts/check.sh`, `cargo build --release`, `cargo build --release --no-default-features`, `cargo build --release --no-default-features --features webkit`, `./scripts/package.sh`, plus install-and-launch smoke checks for the packaged artifact on Jammy and on a newer distro. Add checks that docs/tooltips/feature flags stay aligned with the actual shortcuts and feature behavior, and that runtime prerequisites (`libghostty.so`, Ghostty resources) are satisfied before renderer checks run.
- **validation**: CI or local scripted checks fail if browser support is silently removed, if Jammy build/package/install/runtime support regresses, if packaged artifacts are broken, or if newer-distro builds lose expected behavior.
- **status**: Not Completed
- **log**:
- **files edited/created**:

### T8: Publish the Support Matrix and Release Workflow
- **depends_on**: [T7]
- **location**: `README.md`, new `docs/support/ubuntu-22.04.md`, release notes/template, packaging scripts/workflows
- **description**: Publish the validated Jammy path only after T7 passes. Document package prerequisites, required package channels, known caveats, runtime requirements, and the maintainer workflow for validating releases across Jammy and current distros. The documentation should make it obvious how full parity is achieved without hiding browser or runtime requirements.
- **validation**: A fresh Jammy install of the real release artifact launches both terminal rendering and the built-in browser successfully, a current distro retains the same feature surface, and the published docs match the validated release workflow.
- **status**: Not Completed
- **log**:
- **files edited/created**:

## Parallel Execution Groups

| Wave | Tasks | Can Start When |
|------|-------|----------------|
| 1 | T1 | Immediately |
| 2 | T2 | T1 complete |
| 3 | T3 | T2 complete |
| 4 | T4 | T3 complete |
| 5 | T5, T6 | T4 complete |
| 6 | T7 | T5 complete and T6 complete or explicitly skipped |
| 5 | T8 | T7 complete |

## Testing Strategy
- Reproduce the current tree on Ubuntu 22.04 before changing code.
- Keep the canonical workspace quality gate green with `./scripts/check.sh`.
- Add Jammy-specific build/package/install/runtime smoke checks instead of relying on one maintainer machine.
- Validate default full build, `--no-default-features`, and explicit browser-enabled build paths separately.
- Capture `apt-cache policy`, `pkg-config --modversion`, `cargo tree`, `ldd`, and renderer/runtime logs in the Jammy harness.
- Verify packaged artifacts, not just source-tree builds.
- Verify runtime terminal rendering and built-in browser behavior on Jammy and on a current distro after any GL compatibility change.
- Verify no shortcut, tooltip, README, or packaging regressions are introduced as collateral damage.

## Risks & Mitigations
- **Risk**: Over-correcting by downgrading the whole GTK stack when only feature gates or package assumptions were wrong.
  **Mitigation**: T3 must prove the minimum required change before T4 edits dependencies.
- **Risk**: Hiding Jammy incompatibility by turning real features into no-op flags.
  **Mitigation**: T4 and T7 require every published feature flag to remain honest and buildable.
- **Risk**: Shipping a large unsafe GL workaround that becomes permanent maintenance debt.
  **Mitigation**: T6 is conditional on baseline evidence and must live behind a dedicated module with focused diagnostics and tests.
- **Risk**: Supporting browser parity depends on package channels that some Jammy users may not have enabled.
  **Mitigation**: T1 and T8 must document the exact package/source contract and release prerequisites.
- **Risk**: Ubuntu fixes accidentally change user-facing behavior on other distros.
  **Mitigation**: T7 adds cross-distro build/runtime smoke checks plus product-surface checks for shortcuts, docs, and browser availability.
- **Risk**: The plan could appear complete while Jammy still has partial feature failure.
  **Mitigation**: T8 requires successful terminal rendering and browser launch from the real packaged artifact on a fresh Jammy install before the work counts as done.

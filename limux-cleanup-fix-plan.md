# Plan: Limux Cleanup Fixes

**Generated**: 2026-03-22

## Overview
Revalidate the Limux-only cleanup work against the current rolled-back base and fix the maintainability issues that are still present today. The highest-priority gap is that the Linux host has only the legacy `workspaces.json` persistence path in `window.rs`; the later `session.rs` persistence layer is not present on this base, so the cleanup plan must start by introducing one canonical restore-state module instead of trying to consolidate around an existing implementation. The plan also keeps the host-only cleanup scoped away from Ghostty engine modules and targets currently reproducible strict-clippy failures in `limux-host-linux` and `limux-core`.

## Current Validation Snapshot
- `cargo test -p limux-host-linux --no-default-features`: passes on this base.
- `cargo clippy -p limux-host-linux --no-default-features --tests -- -D warnings`: fails in `pane.rs`, `terminal.rs`, and `window.rs`.
- `cargo clippy -p limux-core --tests -- -D warnings`: fails in `lib.rs`.
- `cargo clippy -p limux-cli --tests -- -D warnings`: currently fails because `limux-core` is not clippy-clean.

## Dependency Graph

```text
T1 ──┬── T3 ──┬── T4
     │        └── T6
T2 ──┘
T5 ───────────────┘
```

## Tasks

### T1: Add Canonical Session Persistence Layer
- **depends_on**: []
- **location**: `rust/limux-host-linux/src/session.rs`, `rust/limux-host-linux/src/main.rs`, `rust/limux-host-linux/src/window.rs`
- **description**: Introduce a dedicated versioned session module for host restore state. Define serializable app/workspace/layout/pane/tab snapshot structs, persist them under the XDG state directory with atomic temp-plus-rename writes, and add tolerant load behavior for missing, corrupt, and unknown-version files. Include one-time migration from the current legacy `workspaces.json` file so the host moves to a single persistence source of truth instead of keeping duplicate save paths alive.
- **validation**: `cargo test -p limux-host-linux session::`

### T2: Replace Ad Hoc Host Runtime State With Typed Pane/Terminal Metadata
- **depends_on**: []
- **location**: `rust/limux-host-linux/src/pane.rs`, `rust/limux-host-linux/src/terminal.rs`
- **description**: Remove the current string-keyed widget-data dependency for pane internals and replace it with first-class typed state that the host can query directly. Use GTK/glib property-backed objects or equivalent typed host-owned structs rather than arbitrary `set_data` storage. While doing that cleanup, collapse callback signatures and context passing so the current `clippy::type_complexity`, `too_many_arguments`, and redundant-local issues are resolved without introducing wrapper layers or Ghostty-side refactors.
- **validation**: `cargo test -p limux-host-linux pane::`; `cargo clippy -p limux-host-linux --no-default-features --tests -- -D warnings`

### T3: Move Window Restore/Autosave Logic Onto Declarative Snapshots
- **depends_on**: [T1, T2]
- **location**: `rust/limux-host-linux/src/window.rs`
- **description**: Replace the current `SavedWorkspace` plus direct widget-tree persistence helpers with snapshot/build logic that round-trips through the new session model. Preserve workspace order, favorite pinning, active workspace, split tree shape, divider ratios, tab metadata, and sidebar state. Add reconstruction guards so restore does not trigger partial autosaves while the UI is being rebuilt, and delete the legacy `workspaces.json` save/load path once migration is wired.
- **validation**: `cargo test -p limux-host-linux window::`; `cargo test -p limux-host-linux --no-default-features`

### T4: Close Current Host Maintainability/Lint Debt
- **depends_on**: [T2, T3]
- **location**: `rust/limux-host-linux/src/pane.rs`, `rust/limux-host-linux/src/terminal.rs`, `rust/limux-host-linux/src/window.rs`
- **description**: Make the host crate fully strict-clippy clean on the current code path. The cleanup should explicitly eliminate the currently reproduced warnings in `pane.rs` (`type_complexity`, `too_many_arguments`, `if_same_then_else`), `terminal.rs` (`type_complexity`, `redundant_locals`), and `window.rs` (`manual_flatten`) using clearer local types/helpers rather than lint suppressions.
- **validation**: `cargo clippy -p limux-host-linux --no-default-features --tests -- -D warnings`

### T5: Refactor Validated `limux-core` Hotspots Only
- **depends_on**: []
- **location**: `rust/limux-core/src/lib.rs`
- **description**: Address the currently failing `limux-core` maintainability issues at the validated hotspots instead of broad speculative extraction. That includes the command-palette command-spec section, the duplicated scoring branch, the `surface_id` early-return block, and the XDG-path membership checks. Keep the work focused on reducing local complexity and making strict clippy pass; do not open a wider rewrite of unrelated core command handling unless new evidence appears during implementation.
- **validation**: `cargo clippy -p limux-core --tests -- -D warnings`; `cargo test -p limux-core`

### T6: End-to-End Workspace Verification
- **depends_on**: [T3, T4, T5]
- **location**: workspace root
- **description**: Re-run the repo checks needed to support the original maintainability claim for Limux-owned code. Confirm the host remains green after the persistence rewrite, confirm `limux-core` is strict-clippy clean, and confirm `limux-cli` is no longer blocked by upstream crate lint failures. If GUI-dependent verification cannot be automated, record the exact manual reopen flow for host persistence validation.
- **validation**: `cargo test -p limux-host-linux --no-default-features`; `cargo clippy -p limux-host-linux --no-default-features --tests -- -D warnings`; `cargo test -p limux-core -p limux-cli`; `cargo clippy -p limux-core -p limux-cli --tests -- -D warnings`; `cargo fmt --check`

## Notes On Scope Changes From The Earlier Plan
- The prior later-stage cleanup plan is not correct for this base because `rust/limux-host-linux/src/session.rs` does not exist here.
- The persistence work must start from introducing the canonical session layer, not from consolidating around an already-present one.
- A standalone `limux-cli` refactor task is not justified by current evidence on this base; the CLI is presently blocked by `limux-core` clippy failures rather than its own reproduced lint debt.
- Ghostty engine/FFI modules remain out of scope unless a host-only task cannot be completed without a narrowly scoped binding change.

## External API Note
- GTK4-rs documentation currently favors first-class `glib::Object` / property-backed state for custom UI data models, which supports replacing arbitrary widget-object data with typed host-owned state during T2.

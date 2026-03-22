# Plan: Workspace Layout Persistence

**Generated**: 2026-03-21

## Overview
Persist Limux workspace restore state across app restarts without attempting to preserve live PTY processes. The implementation will make restore state first-class instead of scraping GTK widgets ad hoc. Restored workspaces must bring back workspace order, favorites, active selection, split layout, split ratios, pane tab sets, active tabs, tab titles, pinned state, terminal working directories, browser URLs, and prior terminal transcript/history captured from Ghostty at shutdown. On reopen, terminal tabs come back as fresh terminals in the same layout with the previous-session transcript visible in the tab.

## Prerequisites
- Rust workspace builds locally from `/home/willr/Applications/limux`
- GTK4/libadwaita host runtime
- Ghostty embedded API already vendored in repo
- `serde` and `serde_json` available from workspace dependencies

## Dependency Graph

```text
T1 ──┬── T3 ──┐
     │        ├── T4
T2 ──┘        │
T5 ───────────┘
```

## Tasks

### T1: Add Versioned Restore Snapshot Model and Persistence IO
- **depends_on**: []
- **location**: `rust/cmux-host-linux/Cargo.toml`, `rust/cmux-host-linux/src/session.rs`, `rust/cmux-host-linux/src/main.rs`
- **description**: Create a dedicated restore-state module with versioned serializable structs for app, workspace, layout tree, pane, and tab restore data. Include sidebar width, workspace names, favorites, active selections, and tolerant per-tab metadata. Add disk IO helpers using GLib/XDG user state storage, atomic temp+rename writes, best-effort lock handling, tolerant load behavior, corrupt-file quarantine, and unknown-version fallback behavior. Wire the new module into the host crate without yet connecting the live UI snapshot builder.
- **validation**: `cargo test -p cmux-host-linux session::` or equivalent module-focused host tests; verify round-trip serialization, version/default handling, corrupted/missing file fallback, and corrupt-file quarantine behavior.
- **status**: Completed
- **log**:
  - Added `src/session.rs` with versioned restore schema, XDG state-dir storage, atomic temp+rename writes, lockfile handling, corrupt quarantine, and tolerant load outcomes.
  - Wired the host crate through `src/main.rs` with a shared `APP_ID`.
  - Added session-layer tests for round-trip serialization, missing version defaults, missing file behavior, corrupt quarantine, unknown-version fallback, and disk persistence.
- **files edited/created**:
  - `rust/cmux-host-linux/src/session.rs`
  - `rust/cmux-host-linux/src/main.rs`

### T2: Refactor Pane/Tab Runtime State for Snapshot + Restore
- **depends_on**: []
- **location**: `rust/cmux-host-linux/src/pane.rs`, `rust/cmux-host-linux/src/terminal.rs`, `rust/cmux-ghostty-sys/src/lib.rs`
- **description**: Replace purely local pane/tab state with first-class runtime metadata that can be queried for save/restore. Add tab-kind metadata for terminal and browser tabs, expose pane snapshot extraction and restore constructors, track current browser URL, and gate default browser loads so restored URLs are not clobbered. Add Ghostty text readback bindings/helpers for full-screen transcript capture at session-save time only, including size cap and safe cleanup of returned buffers. Restored terminal tabs must render previous-session transcript/history together with a fresh live terminal in the same tab. Unknown tab kinds must degrade safely during restore.
- **validation**: `cargo test -p cmux-host-linux pane::` or equivalent targeted tests covering tab snapshot extraction/restore metadata, restored-browser URL preservation, and compile-time validation of new Ghostty FFI readback helpers.
- **status**: Completed
- **log**:
  - Added Ghostty readback bindings and owned-text helpers in `cmux-ghostty-sys`.
  - Refactored pane/tab runtime state to support snapshot extraction and restore constructors, including browser URL tracking, unsupported-tab fallback, and pane mutation callbacks back into the host.
  - Restored terminal tabs now render previous transcript above a fresh live terminal; transcript capture only happens during full-session save.
- **files edited/created**:
  - `rust/cmux-ghostty-sys/src/lib.rs`
  - `rust/cmux-host-linux/src/pane.rs`
  - `rust/cmux-host-linux/src/terminal.rs`

### T3: Add Workspace Tree Snapshot and Restore-Oriented Window State
- **depends_on**: [T1, T2]
- **location**: `rust/cmux-host-linux/src/window.rs`
- **description**: Refactor window/workspace state so the workspace tree can be snapshotted and restored from declarative data instead of implicit GTK structure alone. Add recursive layout extraction/build for nested `gtk::Paned`, preserve split orientation and divider ratios, preserve workspace ordering/favorites/active workspace/sidebar width, and add restore guards so signal-driven autosave does not persist partial state during reconstruction. Structural save triggers must use a transcript-free snapshot path, while full session close uses transcript capture.
- **validation**: `cargo test -p cmux-host-linux window::` or equivalent targeted tests for layout tree round-trip, favorite ordering preservation, ratio restore/clamping, and restore behavior over split trees and tabs.
- **status**: Completed
- **log**:
  - Added recursive split-tree restore/build helpers in `window.rs`, plus snapshot conversion between widget state and session snapshots.
  - Added restore guards so startup reconstruction does not trigger partial autosaves.
  - Structural saves now run from workspace/sidebar mutations and pane/tab/browser metadata changes without transcript capture.
- **files edited/created**:
  - `rust/cmux-host-linux/src/window.rs`

### T4: Startup/Shutdown Integration and End-to-End Restore Verification
- **depends_on**: [T3, T5a, T5b]
- **location**: `rust/cmux-host-linux/src/main.rs`, `rust/cmux-host-linux/src/window.rs`, `workspace-layout-persistence-plan.md`
- **description**: Connect restore-state loading on startup and full-session save on close so reopening Limux reconstructs the last saved layout. Ensure terminal transcript capture happens only at full session save, fresh terminals are created on restore, invalid `cwd` values fall back safely, and missing/corrupt/unknown-version restore files fall back cleanly to a default workspace with a reset path available for users who want a clean start. Update plan logs as implementation completes.
- **validation**: `cargo test -p cmux-host-linux`; `cargo build -p cmux-host-linux` and, if environment permits, `cargo build -p cmux-host-linux --features webkit`; manual smoke verification path documented for open/close/reopen behavior.
- **status**: Completed
- **log**:
  - Startup now loads the last restore snapshot and falls back cleanly for missing, corrupt, unreadable, or unknown-version state.
  - Window close now performs the full transcript-inclusive save.
  - Validation completed with `cargo test -p cmux-host-linux`, `cargo build -p cmux-host-linux`, `cargo build -p cmux-host-linux --features webkit`, and a timed `cargo run -p cmux-host-linux --features webkit --bin cmux-linux` smoke launch that stayed up until the timeout.
- **files edited/created**:
  - `rust/cmux-host-linux/src/window.rs`
  - `workspace-layout-persistence-plan.md`

### T5a: Add Session-Layer Regression Tests
- **depends_on**: [T1]
- **location**: `rust/cmux-host-linux/src/session.rs`, `rust/cmux-host-linux/src/pane.rs`, `rust/cmux-host-linux/src/window.rs`
- **description**: Add pure-data regression tests for the persistence contract at the session layer: snapshot schema versioning, golden JSON compatibility, missing/unknown fields, invalid indices clamping, corrupted-file behavior, and large transcript handling. Keep these tests widget-free.
- **validation**: `cargo test -p cmux-host-linux session::`
- **status**: Completed
- **log**:
  - Added pure-data restore contract tests directly in `session.rs` for schema defaults, corrupt handling, and persistence round-trips.
- **files edited/created**:
  - `rust/cmux-host-linux/src/session.rs`

### T5b: Add Layout and Tab Regression Tests
- **depends_on**: [T2, T3]
- **location**: `rust/cmux-host-linux/src/pane.rs`, `rust/cmux-host-linux/src/window.rs`
- **description**: Add focused regression tests for layout/tree invariants and tab restore behavior: split-tree round-trip, workspace order/favorites, active selections, restored browser URL preservation, and transcript-preservation metadata rules. Keep tests pure-data where possible so they run reliably without a full GTK display harness.
- **validation**: `cargo test -p cmux-host-linux`
- **status**: Completed
- **log**:
  - Added pane/window pure-data tests for browser initial-load behavior, empty-pane normalization, split ratio clamping, working-directory sanitation, unsupported-tab fallback, and custom-title metadata preservation.
- **files edited/created**:
  - `rust/cmux-host-linux/src/pane.rs`
  - `rust/cmux-host-linux/src/window.rs`

## Parallel Execution Groups

| Wave | Tasks | Can Start When |
|------|-------|----------------|
| 1 | T1, T2 | Immediately |
| 2 | T5a, T3 | T1 complete for T5a; T1 and T2 complete for T3 |
| 3 | T5b | T2 and T3 complete |
| 4 | T4 | T3, T5a, and T5b complete |

## Testing Strategy
- Prefer pure-data snapshot/restore tests for session structs, pane snapshots, and split-tree round-trips.
- Add focused unit tests near helper functions to keep GTK runtime exposure small.
- Use golden JSON fixtures/inline cases for backward-compatible load behavior and invalid snapshot repair.
- Run `cargo test -p cmux-host-linux` after integration.
- Run `cargo build -p cmux-host-linux` and the `webkit` build variant if dependencies are available.
- If full GUI reopen automation is not practical in CI, document an exact manual reopen flow and expected outcomes.

## Risks & Mitigations
- Ghostty text readback is expensive: only capture terminal transcript during explicit session save, not on every keystroke.
- GTK widget state is hard to reconstruct from raw widgets: create declarative snapshot/build APIs rather than scraping arbitrary widget trees in many places.
- Restored terminal transcript UX could regress normal terminal usage: keep transcript clearly separated from the fresh terminal session within the tab.
- Save file corruption could block startup: load must be best-effort and fall back to a clean default workspace.
- Browser state tracking can drift from the actual WebView: use URL-notify hooks to keep tab metadata updated instead of reading labels only.
- Restore-time signal churn can trigger partial autosaves: explicitly suspend save-on-change while rebuilding widget state, then re-enable it after restore completes.
- Raw divider pixels do not survive monitor/DPI changes well: persist ratios and clamp on restore.

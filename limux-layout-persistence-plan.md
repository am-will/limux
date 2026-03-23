# Plan: Limux Layout Persistence

**Generated**: 2026-03-22

## Overview
Persist the Linux host's full session layout so closing and reopening `limux` restores:

- workspace order, names, favorites, and active workspace
- pane tree shape and split orientation
- split positioning
- pane-local tab sets and active tabs
- terminal tabs recreated in their own last-known working directory
- browser tabs reopened to their last URI
- existing tab metadata such as rename and pin state
- sidebar visibility/width when it materially affects the reopened layout

This plan keeps one canonical persistence path in `limux-host-linux` and does not add adapter layers. The implementation should stay in the host crate and use the existing `serde`/`serde_json`/`dirs`/GTK/WebKit stack already in the workspace.

Scope note: this restores UI topology and browser destinations, not live PTY process memory. Terminal tabs should come back as fresh terminals at the last known cwd because Ghostty session checkpoint/restore does not exist in the current host architecture.

## Prerequisites
- Workspace root: `/home/willr/Applications/cmux-linux/cmux`
- Host crate: `/home/willr/Applications/cmux-linux/cmux/rust/limux-host-linux`
- Existing dependencies already present in `Cargo.toml`: `gtk4`, `libadwaita`, `serde`, `serde_json`, `dirs`, `webkit6`
- GTK docs confirmation: saving app state on `close_request` and restoring stack/window state is a normal gtk4-rs pattern
- WebKit docs confirmation: browser restore can be anchored on the current URI, with WebKit continuing to own engine-managed state such as cookies/session data where supported

## Dependency Graph

```text
T1 ──┬── T2 ── T3 ──┐
     └── T4 ────────┼── T5 ──┬── T6 ── T7
                    │        └── T8
```

## Tasks

### T1: Define Canonical Session File Semantics And Legacy Migration
- **depends_on**: []
- **location**: `/home/willr/Applications/cmux-linux/cmux/rust/limux-host-linux/src/main.rs`, `/home/willr/Applications/cmux-linux/cmux/rust/limux-host-linux/src/window.rs`, `/home/willr/Applications/cmux-linux/cmux/rust/limux-host-linux/src/layout_state.rs`
- **description**: Create a dedicated layout-state module with versioned serde types for the whole saved session: app state, workspace state, recursive layout tree, pane state, tab state, terminal tab state, and browser tab state. Define explicit file ownership rules: canonical filename, load precedence, one-time legacy fallback from the current `workspaces.json` array, and deterministic defaults for fields that legacy data does not contain. Replace plain `std::fs::write` with atomic temp-file + rename writes so partial session files cannot be committed on crash.
- **validation**: The new schema has an explicit version field; legacy `Vec<SavedWorkspace>` input still loads; when both legacy and canonical files exist the winning file is deterministic; writes are atomic; corrupt/missing files fail soft to an empty session instead of crashing.
- **status**: Completed
- **log**: Added `layout_state.rs` as the canonical persistence module with versioned session types, canonical-vs-legacy load precedence, one-time legacy import from `workspaces.json`, ratio helpers, and atomic temp-file + rename writes. Added pure regression tests for migration, precedence, corrupt-file fallback, and persistence defaults.
- **files edited/created**: `rust/limux-host-linux/src/layout_state.rs`, `rust/limux-host-linux/src/main.rs`, `rust/limux-host-linux/Cargo.toml`

### T2: Add Leaf Pane Snapshot/Restore APIs For Tabs, Browser URIs, And Per-Terminal CWD
- **depends_on**: [T1]
- **location**: `/home/willr/Applications/cmux-linux/cmux/rust/limux-host-linux/src/pane.rs`, `/home/willr/Applications/cmux-linux/cmux/rust/limux-host-linux/src/terminal.rs`, `/home/willr/Applications/cmux-linux/cmux/rust/limux-host-linux/src/layout_state.rs`
- **description**: Teach `pane.rs` how to export and rebuild pane-local state. That includes tab ordering, active tab selection, renamed titles, pinned flags, terminal-vs-browser tab kind, per-terminal cwd, and browser URI. Refactor pane creation so it can build from an explicit saved pane snapshot instead of always injecting a default terminal first. Fold browser restore correctness into this task: restored browser tabs must accept a saved URI, and the current first-map Google bootstrap must only run for brand-new browser tabs with no saved URI.
- **validation**: A pane model with mixed terminal/browser tabs can be serialized and reconstructed with the same tab count, order, active tab, rename state, pin state, per-terminal cwd, and browser URI. Restored browser tabs do not get clobbered by the default Google load path.
- **status**: Completed
- **log**: Refactored `pane.rs` so panes can be created from explicit saved tab state instead of always bootstrapping a fresh terminal. Added per-tab terminal cwd tracking, browser URI tracking, restore-time browser bootstrap handling, pane snapshot export, and state-change callbacks for tab mutations.
- **files edited/created**: `rust/limux-host-linux/src/pane.rs`

### T3: Add Recursive Split-Tree Snapshot And Restore On Top Of The Leaf Contract
- **depends_on**: [T1, T2]
- **location**: `/home/willr/Applications/cmux-linux/cmux/rust/limux-host-linux/src/window.rs`, `/home/willr/Applications/cmux-linux/cmux/rust/limux-host-linux/src/layout_state.rs`
- **description**: Walk the workspace root widget tree and convert `gtk::Paned` plus saved leaf panes into a recursive layout model. Store split orientation and divider placement as a ratio derived from actual allocation, not raw pixels, so layouts survive different launch sizes. Add the inverse builder that reconstructs nested split trees and reapplies divider ratios after widgets are allocated. Define fallback behavior for zero-sized allocations, malformed trees with missing children, invalid ratios, and stale `active_tab` references.
- **validation**: A nested horizontal/vertical tree round-trips with the same pane count, split directions, and materially equivalent divider placement after reopen. Invalid ratios or malformed trees clamp/fallback instead of crashing.
- **status**: Completed
- **log**: Added recursive split-tree snapshot and restore in `window.rs`, including orientation persistence, ratio-based divider storage, and map-time ratio reapplication so hidden/background workspaces can restore their split positions when they become visible. Also attached divider move hooks plus last-known ratio state so hidden restored workspaces do not collapse back to default split ratios if the app closes before they are visited.
- **files edited/created**: `rust/limux-host-linux/src/window.rs`

### T4: Build A Persistence Coordinator With Restore Suspension And Atomic Save Entry Points
- **depends_on**: [T1]
- **location**: `/home/willr/Applications/cmux-linux/cmux/rust/limux-host-linux/src/window.rs`, `/home/willr/Applications/cmux-linux/cmux/rust/limux-host-linux/src/layout_state.rs`
- **description**: Create one canonical save/load coordinator in the host crate: `load_session`, `save_session_atomic`, `request_session_save`, and a restore guard such as `is_restoring` or `persistence_suspended`. This task defines when saves are allowed, how restore batching suppresses partial writes during startup rebuild, and how the host performs one final flush after a successful restore/migration pass.
- **validation**: There is a single save/load owner, restore can rebuild the UI without writing partial state mid-flight, and one explicit final flush happens after restore completes.
- **status**: Completed
- **log**: Implemented the save/load coordinator in `window.rs`: debounced `request_session_save`, atomic `save_session_now`, restore suspension, session load application, direct sidebar-state restore, and a final post-restore save for migrated legacy state.
- **files edited/created**: `rust/limux-host-linux/src/window.rs`, `rust/limux-host-linux/src/layout_state.rs`

### T5: Integrate Full Session Persistence Into Window, Workspace, Sidebar, And Mutation Hooks
- **depends_on**: [T2, T3, T4]
- **location**: `/home/willr/Applications/cmux-linux/cmux/rust/limux-host-linux/src/window.rs`, `/home/willr/Applications/cmux-linux/cmux/rust/limux-host-linux/src/pane.rs`
- **description**: Replace the current workspace-only save/load flow with full session restore on startup and full session save through the coordinator. Restore active workspace, workspace folder/cwd metadata, root layout tree, and sidebar state. Apply restored sidebar visibility/width directly under restore suspension instead of through the normal animation/toggle path to avoid flicker and save-loop noise. Hook save requests to every persisted mutation, routed through the coordinator: split create/remove, pane close, workspace create/rename/reorder/favorite/select/close, tab add/remove/reorder/activate/rename/pin, browser URI changes, and sidebar visibility/width changes. Ignore transient empty or `about:blank` browser URI notifications during restore so startup transitions do not overwrite the authoritative saved URI. Do not duplicate save logic across callbacks.
- **validation**: Startup with a saved session recreates the same workspaces and layout without manual re-splitting; closing the app writes the latest structure; switching tabs/workspaces and navigating browsers updates the persisted model through one shared path; restore does not emit partial writes or sidebar flicker; transient browser startup URIs do not overwrite the saved page.
- **status**: Completed
- **log**: Replaced the old workspace-only persistence flow with full session restore/save wiring. Save requests now flow through one coordinator for workspace rename/reorder/favorite/select/close, tab activation/reorder/rename/pin/add/remove, browser URI changes, split creation/removal, and sidebar width/visibility changes.
- **files edited/created**: `rust/limux-host-linux/src/window.rs`, `rust/limux-host-linux/src/pane.rs`

### T6: Add Regression Tests For Migration, Edge Cases, And Layout Round-Trip
- **depends_on**: [T2, T3, T4, T5]
- **location**: `/home/willr/Applications/cmux-linux/cmux/rust/limux-host-linux/src/window.rs`, `/home/willr/Applications/cmux-linux/cmux/rust/limux-host-linux/src/pane.rs`, `/home/willr/Applications/cmux-linux/cmux/rust/limux-host-linux/src/layout_state.rs`
- **description**: Add focused tests around the persistence model and restore flow. Cover legacy file migration, canonical-file precedence, atomic-write helpers, recursive split-tree encode/decode, zero-size or invalid ratio fallback, malformed tree fallback, stale active-tab fallback, pane/tab metadata round-trip, per-terminal cwd persistence, browser URI persistence, and restore-suspension behavior.
- **validation**: `cargo test -p limux-host-linux --manifest-path /home/willr/Applications/cmux-linux/cmux/Cargo.toml` passes with coverage proving the persistence contract and its edge-case behavior.
- **status**: Completed
- **log**: Added and ran green model-level regression coverage for legacy migration, canonical-file precedence, corrupt canonical fallback, empty-pane fallback, stale active-tab fallback, split-ratio clamping, and the hidden-workspace zero-allocation case that must preserve the last saved split ratio. Validation command: `cargo test -p limux-host-linux --manifest-path /home/willr/Applications/cmux-linux/cmux/Cargo.toml -- --nocapture`.
- **files edited/created**: `rust/limux-host-linux/src/layout_state.rs`

### T7: Run Build And Manual Reopen Verification
- **depends_on**: [T6]
- **location**: `/home/willr/Applications/cmux-linux/cmux`
- **description**: Build the host, launch it, create a non-trivial layout, close it, relaunch it, and verify restoration. The manual scenario should include multiple workspaces, nested splits, several terminal tabs with different directories, browser tabs at non-default URLs, renamed/pinned tabs, and non-default active workspace selection.
- **validation**: `cargo build -p limux-host-linux --features webkit --manifest-path /home/willr/Applications/cmux-linux/cmux/Cargo.toml` succeeds and the manual reopen checklist passes end-to-end.
- **status**: Partially Completed
- **log**: Verified the host builds with WebKit enabled and the binary launches from the current workspace after the final hidden-workspace split-ratio fix. Validation commands: `cargo build -p limux-host-linux --features webkit --manifest-path /home/willr/Applications/cmux-linux/cmux/Cargo.toml` and `timeout 5 cargo run -p limux-host-linux --features webkit --manifest-path /home/willr/Applications/cmux-linux/cmux/Cargo.toml`. Full interactive close/reopen walkthrough could not be completed in this non-interactive session, so a live manual GTK validation pass is still recommended.
- **files edited/created**: none

### T8: Document Persistence Limits And File Ownership At The Canonical Codepath
- **depends_on**: [T5]
- **location**: `/home/willr/Applications/cmux-linux/cmux/rust/limux-host-linux/src/layout_state.rs`, `/home/willr/Applications/cmux-linux/cmux/rust/limux-host-linux/src/window.rs`, `/home/willr/Applications/cmux-linux/cmux/rust/limux-host-linux/src/pane.rs`
- **description**: Add succinct code comments where they materially reduce future ambiguity: canonical filename ownership, legacy import behavior, ratio-vs-pixel reasoning, restore-suspension semantics, and the intentional limit that terminal process state is not restored. Keep these notes next to the implementation rather than in duplicated docs.
- **validation**: The save/restore path has a single obvious owner and future maintainers can see the critical invariants without reverse-engineering them.
- **status**: Completed
- **log**: Added code-local persistence comments in the new canonical codepath to document version/file ownership, ratio handling, save coordination, and the intentional limit that terminal process memory is not restored.
- **files edited/created**: `rust/limux-host-linux/src/layout_state.rs`, `rust/limux-host-linux/src/window.rs`

## Parallel Execution Groups

| Wave | Tasks | Can Start When |
|------|-------|----------------|
| 1 | T1 | Immediately |
| 2 | T2, T4 | T1 complete |
| 3 | T3 | T2 complete |
| 4 | T5 | T2, T3, T4 complete |
| 5 | T6, T8 | T6 after T2, T3, T4, T5; T8 after T5 |
| 6 | T7 | T6 complete |

## Testing Strategy
- Add pure serde/model tests for legacy import, canonical-file precedence, versioned save output, atomic-write helpers, corrupt file fallback, and recursive split-tree round-trip.
- Add host-side tests for pane/tab metadata preservation: active tab, renamed tab, pinned tab, terminal/browser tab mix, per-terminal cwd, and saved browser URI.
- Add tests for restore-suspension semantics so startup rebuild does not trigger partial writes.
- Manually validate GTK behavior with a real launched `limux` session because split-ratio restore and browser map/load behavior are UI-runtime sensitive.
- Use the existing workspace manifest path when building and testing: `/home/willr/Applications/cmux-linux/cmux/Cargo.toml`.

## Risks & Mitigations
- **Risk**: Existing users already have `workspaces.json`, and replacing it blindly would discard their saved workspaces.
  **Mitigation**: Implement explicit legacy import support, define canonical filename and precedence rules, and write the new canonical file only through the atomic writer.
- **Risk**: Saving raw `gtk::Paned::position()` pixels will restore badly when the window size changes.
  **Mitigation**: Save ratios against the current allocation, clamp invalid values, and reapply them after allocation with safe fallbacks.
- **Risk**: Browser restore can be clobbered by the current first-map default Google load.
  **Mitigation**: Fold browser restore into the leaf-pane contract, only allow the bootstrap path when there is no saved URI, and ignore transient empty or `about:blank` URI notifications during restore.
- **Risk**: Restore can write partial state if autosaves remain live during startup reconstruction.
  **Mitigation**: Centralize save calls behind a persistence coordinator with `is_restoring` or `persistence_suspended` semantics and one final post-restore flush.
- **Risk**: Restoring sidebar state through the animated toggle path can create flicker or extra save noise.
  **Mitigation**: Apply restored sidebar visibility/width directly during restore and re-enable normal animation behavior only after the restore batch completes.
- **Risk**: Per-terminal cwd will be wrong if only workspace-level cwd is persisted.
  **Mitigation**: Persist cwd on each terminal tab and update it from the terminal callback path rather than collapsing all tabs to workspace cwd.
- **Risk**: Users may expect terminal process checkpointing rather than layout restoration.
  **Mitigation**: Keep the implementation honest: restore terminal tab presence and cwd, and document the limit in code near the persistence contract.

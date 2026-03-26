# Plan: Clean Tab Drag-and-Drop Port For Limux Host

**Generated**: 2026-03-25

## Overview
Port the UX from PR 17 into the current `limux-host-linux` codebase without importing its regressions or duplicating code paths. The target feature set is:

- Reorder tabs within a pane with insertion semantics
- Move tabs across panes
- Split panes by dropping a tab on content edges
- Move tabs to other workspaces from the sidebar
- Create a new workspace by dropping a tab on `New Workspace`
- Keep hover/switch visual feedback and workspace-row targeting

The implementation should not be a blind cherry-pick. The current host already has working pane/session/sidebar abstractions, plus additional browser/shortcut behavior not present in the PR base. Build this as one canonical tab-transfer system rooted in `pane.rs`, with `window.rs` only coordinating workspace/split topology and persistence.

Key design corrections versus PR 17:

- Keep tab identity stable across moves
- Move whole tab state through one canonical transfer path instead of rebuilding partial tab state ad hoc
- Preserve live terminal/browser title updates when a tab changes panes
- Route all reorders and moves through one persistence-aware helper so restart state is always correct
- Support empty-pane creation for split/new-workspace drops so moved tabs do not land beside an accidental fallback terminal
- Keep sidebar workspace dragging and tab dragging explicitly separate to avoid handler conflicts

## Prerequisites
- Workspace root: `/home/willr/Applications/cmux-linux/cmux`
- Current host modules:
  - `/home/willr/Applications/cmux-linux/cmux/rust/limux-host-linux/src/pane.rs`
  - `/home/willr/Applications/cmux-linux/cmux/rust/limux-host-linux/src/window.rs`
  - `/home/willr/Applications/cmux-linux/cmux/rust/limux-host-linux/src/layout_state.rs`
- Existing tests are thin for pane movement; extract pure helpers where needed so regression checks do not depend on live GTK DnD interaction
- Fresh docs lookup was performed against `gtk4-rs` via Context7; the code should continue using repo-native GTK patterns (`DragSource`, `DropTarget`, `Overlay`, `WidgetPaintable`, `set_can_target`)
- Runtime verification should use the Ghostty library path from the current repo README:
  - `LD_LIBRARY_PATH=/home/willr/Applications/cmux-linux/cmux/ghostty/zig-out/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}`
- The worktree is already dirty, especially in [`window.rs`](/home/willr/Applications/cmux-linux/cmux/rust/limux-host-linux/src/window.rs); implementation must stay scoped and avoid broad resets/reverts

## Dependency Graph

```text
T1 ── T2 ── T3 ── T4 ── T5 ── T6 ── T7 ── T8
```

## Tasks

### T1: Establish Shared Drag-And-Drop Foundations
- **depends_on**: []
- **location**: `/home/willr/Applications/cmux-linux/cmux/rust/limux-host-linux/src/pane.rs`, `/home/willr/Applications/cmux-linux/cmux/rust/limux-host-linux/src/window.rs`
- **description**: Add the shared infrastructure needed by every later task before any UX wiring begins. This includes:
  - pane registry/lookup helpers
  - an explicit tab-drag payload type with centralized encode/decode helpers
  - shared drag-session state and listeners
  - cleanup hooks for drag begin/end/cancel paths
  - overlay host scaffolding in `PaneInternals` for both strip indicators and content-edge zones
  - callback plumbing for cross-pane moves and split-with-tab operations
  - destination-resolution interfaces that `window.rs` can use for workspace targets

  Keep payload parsing and drag lifecycle state in one place so later tasks are not coding against raw strings or ad hoc shared flags.
- **validation**: Pure tests cover payload encode/decode and drag-session helper behavior; `cargo test -p limux-host-linux` stays green after the foundation refactor.
- **status**: Not Completed
- **log**:
- **files edited/created**:

### T2: Implement One Canonical Tab Transfer Engine
- **depends_on**: [T1]
- **location**: `/home/willr/Applications/cmux-linux/cmux/rust/limux-host-linux/src/pane.rs`, `/home/willr/Applications/cmux-linux/cmux/rust/limux-host-linux/src/window.rs`
- **description**: Refactor pane-local tab mutation into a single transfer/reorder engine that handles:
  - same-pane reorder with insertion semantics
  - cross-pane move
  - active-tab updates in both source and destination panes
  - source-empty cleanup for “move last tab out of pane/workspace”
  - pinned/custom-name/browser-uri/cwd preservation
  - stable tab ids across moves
  - exactly one persistence notification per completed operation

  Do not duplicate separate helpers for strip moves, sidebar moves, split moves, or workspace moves. Those paths should all call the same engine. The contract here should be explicit: move the whole `TabEntry` intact unless a narrower helper is proven necessary. That avoids the PR 17 title-binding regression and keeps browser/terminal runtime state attached to the moved tab.
- **validation**: Add unit-testable helpers for insert-index computation, source/target active-tab selection, and source-empty collapse policy; tests prove no-op self-drops do not churn state, moving the last tab collapses the source pane/workspace correctly, completed moves emit the expected persistence hook once, and failed/invalid/self-drop paths do not request a save.
- **status**: Not Completed
- **log**:
- **files edited/created**:

### T3: Wire Tab Strip Drag-And-Drop UX
- **depends_on**: [T2]
- **location**: `/home/willr/Applications/cmux-linux/cmux/rust/limux-host-linux/src/pane.rs`
- **description**: Add the visible strip-level drag UX:
  - drag source on movable tab buttons
  - insertion indicator overlay above the tab strip
  - drop targets for same-pane reorder and cross-pane move
  - drag icon via `WidgetPaintable`
  - explicit exclusions for non-movable tabs if needed
  - full drag cleanup on leave/cancel/end

  Keep this layer UI-only. It should compute target position and delegate the actual mutation to T2’s canonical transfer engine. Avoid introducing a second “move by button reparent” path.
- **validation**: Targeted tests cover insert-index math for before/after/end-of-strip cases plus invalid/self-drop no-ops; manual run confirms same-pane reorder and pane-to-pane strip moves behave like Chrome tabs and leave no stale indicator CSS after cancel.
- **status**: Not Completed
- **log**:
- **files edited/created**:

### T4: Add Content-Area Drop Zones And Split-On-Drop
- **depends_on**: [T3]
- **location**: `/home/willr/Applications/cmux-linux/cmux/rust/limux-host-linux/src/pane.rs`, `/home/willr/Applications/cmux-linux/cmux/rust/limux-host-linux/src/window.rs`, `/home/willr/Applications/cmux-linux/cmux/rust/limux-host-linux/src/layout_state.rs`
- **description**: Implement content-area overlays and pane-splitting via tab drop:
  - center drop moves a tab into the target pane
  - left/right edge drop creates a horizontal split
  - top/bottom edge drop creates a vertical split
  - topology helpers explicitly support left/top vs right/bottom placement
  - overlay visuals render above terminal/browser content
  - browser/webview hit-testing is disabled only during active tab drags if required
  - focus and ancestor `Paned` state are cleared safely before widget reparenting

  Split creation must support an intentionally empty destination pane so the dropped tab is the only initial tab in the new pane. This is not a later cleanup item; the left/right/top/bottom split helpers must be correct before the drop overlay is wired.
- **validation**: Helper tests cover drop-zone classification and split placement/orientation; manual validation confirms edge drops create the correct side/orientation and center drops do not create extra panes or focus warnings.
- **status**: Not Completed
- **log**:
- **files edited/created**:

### T5: Extend Sidebar And New-Workspace Targets For Tab Drags
- **depends_on**: [T4]
- **location**: `/home/willr/Applications/cmux-linux/cmux/rust/limux-host-linux/src/window.rs`, `/home/willr/Applications/cmux-linux/cmux/rust/limux-host-linux/src/pane.rs`, `/home/willr/Applications/cmux-linux/cmux/rust/limux-host-linux/src/layout_state.rs`
- **description**: Upgrade existing workspace-row drag/drop so it cleanly supports both workspace reorder drags and tab drags:
  - workspace rows accept tabs and move them into a target workspace pane
  - row hover can switch to that workspace after a delay while dragging a tab
  - `New Workspace` accepts tab drops and creates a new workspace for the tab
  - tab-drop visual states stay distinct from workspace-reorder visual states
  - workspace drag state is broadcast to panes so tab-drop targets can suppress themselves during workspace reorders
  - pending hover timers and drag visuals are cleaned up on leave/cancel/end

  Keep all workspace creation/move logic centralized in `window.rs`; sidebar handlers should parse intent and dispatch to shared helpers. Use one deterministic destination rule for drops onto an existing workspace: choose a stable leaf from `ws.root` using the existing pane-tree traversal helper pattern rather than introducing per-workspace focus history in this feature. Tests must cover drops into a workspace that already has a split tree. Also define new-workspace-from-tab policy explicitly:
  - terminal tab: inherit tab `cwd` for `cwd` and `folder_path` when appropriate
  - browser/keybinds tab: inherit source workspace `cwd` but do not invent a misleading folder path
  - workspace naming: deterministic default derived from tab title with safe fallback

  `New Workspace` must create an empty destination pane for the incoming tab rather than seeding a fallback terminal first.
- **validation**: Tests cover hover/reorder helpers and deterministic workspace destination selection where extractable; manual validation confirms tab-to-workspace, hover-to-switch, and tab-to-new-workspace all work without interfering with workspace reorder/delete flows or creating an extra fallback tab.
- **status**: Not Completed
- **log**:
- **files edited/created**:

### T6: Audit Persistence, Snapshot, And Session Compatibility
- **depends_on**: [T5]
- **location**: `/home/willr/Applications/cmux-linux/cmux/rust/limux-host-linux/src/pane.rs`, `/home/willr/Applications/cmux-linux/cmux/rust/limux-host-linux/src/window.rs`, `/home/willr/Applications/cmux-linux/cmux/rust/limux-host-linux/src/layout_state.rs`
- **description**: Reconcile the now-wired drag/drop behavior with session persistence and restore compatibility:
  - every successful move path requests a session save
  - snapshot/restore remains compatible with existing layouts
  - moved tabs retain serialized browser URI, cwd, custom name, and pin state
  - no transient implementation-only empty-pane state leaks into persisted session files unless explicitly supported
  - `normalize_layout` and fallback helpers do not silently reintroduce the regressions fixed in T2-T5

  By this point the feature behavior already exists; T6 is the compatibility/persistence audit that ensures restart behavior and session normalization match the new model.
- **validation**: Add or extend tests around `PaneState::fallback`, snapshot helpers, and any new empty-pane/session helpers; manual restart check confirms drag/drop layout survives relaunch.
- **validation**: Add or extend tests around `PaneState::fallback`, snapshot helpers, and any new empty-pane/session helpers; manual restart check confirms drag/drop layout survives relaunch and failed/invalid/self-drop paths do not change persisted session state.
- **status**: Not Completed
- **log**:
- **files edited/created**:

### T7: Build Regression Coverage Around The New Canonical Path
- **depends_on**: [T6]
- **location**: `/home/willr/Applications/cmux-linux/cmux/rust/limux-host-linux/src/pane.rs`, `/home/willr/Applications/cmux-linux/cmux/rust/limux-host-linux/src/window.rs`, `/home/willr/Applications/cmux-linux/cmux/rust/limux-host-linux/src/layout_state.rs`
- **description**: Add focused regression coverage for the bug-prone logic instead of trying to fake full GTK drag sessions. Extract pure helpers where necessary so tests can verify:
  - insertion order for reorder math
  - source/destination active-tab selection after move
  - stable metadata preservation across transfers
  - empty-pane/new-workspace creation does not add an unwanted fallback tab
  - persistence callback behavior is not skipped for cross-pane/workspace moves
  - split-on-drop chooses the right orientation/placement helper
  - invalid payloads and cancelled drags leave no stale CSS or drag state
  - moving the last tab out of a pane collapses the source pane instead of serializing an empty pane
  - moving the last tab out of a workspace closes that workspace instead of creating a fallback terminal
  - dropping onto a workspace with an existing split tree lands in the documented destination pane
  - the three reviewed regressions stay fixed

  Keep tests small and grep-friendly; do not hide core logic inside untestable widget closures.
- **validation**: `cargo test -p limux-host-linux` passes with new regression cases that fail against the reviewed PR approach.
- **status**: Not Completed
- **log**:
- **files edited/created**:

### T8: End-To-End Verification And Runtime Check
- **depends_on**: [T7]
- **location**: `/home/willr/Applications/cmux-linux/cmux`
- **description**: Run the full verification loop from this workspace root:
  - targeted unit tests and full crate tests
  - WebKit-enabled host build
  - manual runtime probe of the GTK host for drag/drop UX

  Exercise each shipped feature:
  - reorder within pane
  - move between panes
  - split by edge drop
  - move to existing workspace
  - move to new workspace
  - hover-switch during sidebar drop
  - drag cancel and invalid/self-drop no-op paths
  - moving the last tab out of a pane/workspace
  - dropping into a workspace that already has a split tree
  - restart persistence
- **validation**: `cargo test -p limux-host-linux`; `cargo build -p limux-host-linux --features webkit`; `LD_LIBRARY_PATH=/home/willr/Applications/cmux-linux/cmux/ghostty/zig-out/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH} cargo run -p limux-host-linux --features webkit --bin limux`; manual checklist completed with any residual issues called out explicitly.
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
| 5 | T5 | T4 complete |
| 6 | T6 | T5 complete |
| 7 | T7 | T6 complete |
| 8 | T8 | T7 complete |

## Testing Strategy
- Prefer extracting pure helpers for payload parsing, insert-index computation, zone classification, active-tab selection, and deterministic workspace destination selection so behavior can be regression-tested without simulating GTK DnD end-to-end.
- Keep `pane.rs` as the single source of truth for tab mutation; test the canonical helpers there instead of duplicating assertions across sidebar/split paths.
- Add `window.rs` tests only for workspace-specific policy and helper behavior:
  - workspace drag vs tab drag routing
  - hover/switch timing helper behavior if extracted
  - workspace destination selection for split-tree targets
  - new-workspace-from-tab creation path does not seed an extra fallback tab
- Use existing session snapshot helpers to verify persisted state after move/split/workspace transfers.
- Make the regression suite prove the reviewed PR bugs stay fixed: stable live title updates after cross-pane moves, immediate persistence after pane/workspace moves, and no extra fallback tab on new-workspace drops.
- Finish with a manual GTK runtime pass because browser/webview hit-testing and drag affordances are difficult to prove purely through unit tests.

## Risks & Mitigations
- **Risk**: Rebuilding moved tab widgets disconnects live title updates.
  - **Mitigation**: Move whole tab state intact where possible, and if any widget rebuild is unavoidable, make rebinding a first-class tested helper rather than an implicit side effect.
- **Risk**: Different move entrypoints diverge and persistence becomes inconsistent.
  - **Mitigation**: All reorder/move/split/workspace operations must call one canonical tab-transfer helper that owns save notification semantics.
- **Risk**: Creating a workspace or split target for a dropped tab injects an unwanted fallback terminal.
  - **Mitigation**: Support intentional empty-pane creation for drop destinations and test it directly.
- **Risk**: Workspace drop targets become ambiguous once a workspace already contains a split tree.
  - **Mitigation**: Define one deterministic destination policy in `window.rs` and test it against pre-split workspace layouts.
- **Risk**: Browser/WebKit content swallows drag events.
  - **Mitigation**: Use an overlay/drop-target strategy and, if necessary, temporarily disable hit targeting on the WebView only while a tab drag is active.
- **Risk**: Focus warnings or transient GTK breakage during widget reparenting.
  - **Mitigation**: Clear focused descendants and ancestor `Paned` focus children before stack removal, then reactivate the destination tab after insertion.
- **Risk**: Cancelled or invalid drags leave stale hover timers, CSS, or disabled targetability behind.
  - **Mitigation**: Put cleanup hooks in the shared drag-session layer and require all drag-end/cancel paths to restore sidebar/button/webview state.
- **Risk**: Current local edits in `window.rs` create merge pressure.
  - **Mitigation**: Port the PR feature-by-feature into the current code, avoid wholesale cherry-picks, and keep the diff limited to the pane/workspace DnD surfaces.

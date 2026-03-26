# Plan: T4 Dependency Policy Enforcement

**Generated**: 2026-03-23

## Overview
Confirm that the existing dependency policy already satisfies the Jammy audit and document that decision so downstream maintainers understand why no crate downgrade occurred. Preserve the full-featured default, keep the `webkit` feature path real, and log the outcome inside the overall support plan. This work touches docs plus lightweight validation to prove `cargo build` still produces the browser path.

## Prerequisites
- Familiarity with `docs/internal/jammy-gtk-webkit-ghostty-audit.md`
- Working Rust toolchain (stable) and the ability to run `cargo build`
- Access to README/packaging docs for documentation updates

## Dependency Graph
```
T1 ── T2 ── T3 ── T4
```

## Tasks

### T1: Confirm audit findings
- **depends_on**: []
- **location**: docs/internal/jammy-gtk-webkit-ghostty-audit.md
- **description**: Re-read the Jammy compatibility audit to capture the evidence that GTK4, libadwaita, and WebKit versions already match Jammy packages and that Ghostty packaging, not dependency churn, is the blocker. Record the key points to cite when justifying the current policy.
- **validation**: A short summary (this plan or README note) explicitly states that no version changes are needed because Jammy ships matching packages.
- **status**: Not Completed
- **log**:
- **files edited/created**:

### T2: Document the dependency policy
- **depends_on**: [T1]
- **location**: rust/limux-host-linux/Cargo.toml; README.md (system dependencies / packaging notes); scripts/package.sh (reference to Ghostty resources)
- **description**: Explain in the manifest and README that the default GTK/WebKit stack already works on Jammy, that the `webkit` feature still enables a real browser build, and that Ghostty resources must be packaged. Mention the audit doc or its conclusions so the policy has traceable justification.
- **validation**: README shows a dedicated paragraph referencing `libwebkitgtk-6.0-dev` availability on Jammy and the manifest mentions the audit. No features are removed or turned into no-ops.
- **status**: Not Completed
- **log**:
- **files edited/created**:

### T3: Validate the existing dependency path
- **depends_on**: [T2]
- **location**: cargo build invocations (`cargo build --release` and `cargo build --release --no-default-features --features webkit`)
- **description**: Run the default release build (browser path) and the explicit `--no-default-features --features webkit` build to prove the feature gate remains functional. Keep the commands lightweight.
- **validation**: Both commands succeed locally and the `webkit` feature build still compiles the WebKit-enabled target.
- **status**: Not Completed
- **log**:
- **files edited/created**:

### T4: Update the master plan entry for T4
- **depends_on**: [T2, T3]
- **location**: ubuntu-22-04-full-parity-support-plan.md
- **description**: Fill in the `status`, `log`, and `files edited/created` fields for T4 to describe the documentation plus validation work and to cite the audit justification.
- **validation**: The plan entry clearly states that dependency policy was preserved based on the audit, references the files touched, and shows validation commands.
- **status**: Not Completed
- **log**:
- **files edited/created**:

## Testing Strategy
- Run `cargo build --release` to prove the default (browser) path still compiles.
- Run `cargo build --release --no-default-features --features webkit` to prove the optional `webkit` feature is real.
- No new automated tests required; documentation updates serve as verification of policy.

## Risks & Mitigations
- **Risk**: Accidentally implying a downgrading of GTK/WebKit or injecting fake compatibility flags would confuse maintainers.
  **Mitigation**: Keep all manifests/features unchanged and document the audit note as the justification.
- **Risk**: Documentation claims could drift from real package availability.
  **Mitigation**: Reference concrete package versions from the audit and keep the README/packaging notes aligned with the audit file.

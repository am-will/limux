# Visual smoke captures

`scripts/tests/visual-smoke-test.sh` launches the real GTK host under a private
Xvfb display and D-Bus session, then uses `limux-cli` to verify live terminal
health, output readback, and split panes. It saves wide, narrow, populated, and
split-window screenshots in an HTML gallery and contact sheet. Builds with
workspace-group commands also capture expanded and collapsed sidebar states.
Assertions check runtime behavior and image dimensions; they do not compare
pixels or depend on exact fonts.

Build Limux and Ghostty first, then run:

```bash
LIMUX_VISUAL_SMOKE_ARTIFACT_DIR=/tmp/limux-visual-smoke \
  ./scripts/tests/visual-smoke-test.sh
```

To exercise an existing build from a separate checkout while using the trusted
harness in this tree, set `LIMUX_TEST_ROOT=/path/to/checkout`. The source root
must contain executable `target/debug/limux` and `target/debug/limux-cli`, plus
Ghostty's `zig-out` library and resources. `LIMUX_TEST_HOST` and
`LIMUX_TEST_CLI` can override the binary paths individually.

The `Rust Quality` workflow uploads the gallery on success and failure. For
PRs that need a run before the harness is present in their branch, dispatch the
workflow on `main` with the PR number and its exact current head SHA. The
trusted dispatch job checks out the harness from `main`, builds the requested
PR source without stored credentials, and records the PR head SHA in the run.
The trusted `Visual smoke PR comment` workflow then links the downloadable
artifact in a PR comment only if that head is still current. Download the zip
and open `index.html` to view the gallery; GitHub Actions artifacts do not
provide inline image hosting.

For example, dispatch PR #240 at its current head with:

```bash
gh workflow run rust-quality.yml --repo am-will/limux --ref main \
  -f pull_request_number=240 \
  -f pull_request_sha=YOUR_40_CHARACTER_PR_HEAD_SHA
```

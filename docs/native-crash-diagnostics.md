# Native GTK crash diagnostics

The GTK CI jobs retain `limux-native-crashes` for seven days. Test commands run
normally with core dumps enabled for their descendants; they are not run under
a debugger. Their original exit status still fails the job.

The artifact includes cores, all-thread GDB backtraces, matching executable and
shared-library files in `objects.tar.gz`, SHA-256 hashes, the source revision,
and Ghostty build provenance. `commands.jsonl` records each wrapped command and
its start time without dumping the environment. GDB auto-loading and network
symbol downloads are disabled. CI installs libc debug symbols and requests
`LIMUX_GHOSTTY_DEBUG_SYMBOLS=1`. The current build wrapper retains debug information with
`-Dstrip=false` while keeping `ReleaseFast` optimization. The build verifies the
ELF debug sections before recording the artifact hash. Default release builds
retain their existing flags. Optimized-out frames and variables can still occur.
Manual visual dispatch can target a PR whose older build wrapper does not support
the option. `ghostty_debug_info` in the manifest reports the actual ELF sections;
such an older library can remain stripped even though capture succeeds.

Capture is limited to 4 GiB per core. Collection analyzes at most two cores,
limits matching objects to 2 GiB, and gives each GDB invocation one minute.
`manifest.json` reports missing executables, incomplete cores, and omitted
objects as collection errors. The collector also fails its CI step when a
capture cannot be completed. The raw core and partial backtrace remain available.

An intentional threaded SIGABRT fixture verifies the kernel capture path before
the GTK tests. Its separate `self-test-*` directory proves that the crashing and
waiting threads have source locations and that the matching objects were saved.
It is not a Limux failure. Cores contain test-process memory; the workflow uses
isolated smoke fixtures rather than a user's running terminal session.

For local collector validation without changing `kernel.core_pattern`:

```sh
./scripts/tests/test-native-crash-diagnostics.sh --gdb
```

To inspect a downloaded crash, extract `objects.tar.gz` into an empty directory
and use it as GDB's sysroot. Select the executable path recorded in
`manifest.json`, relative to that directory:

```sh
mkdir -p objects
tar -xzf objects.tar.gz -C objects
gdb -nx -nh -iex 'set auto-load off' -iex 'set debuginfod enabled off' \
  -iex "set sysroot $PWD/objects" \
  -iex "set debug-file-directory $PWD/objects/usr/lib/debug" \
  objects/PATH/TO/EXECUTABLE cores/CORE_FILE
```

The setup command changes the kernel core pattern only on a GitHub-hosted
runner. It refuses workstation use. A passing rerun does not identify the cause
of an earlier allocator abort; preserve the artifact from the failing run.

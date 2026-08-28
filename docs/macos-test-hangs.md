# Diagnosing `make macos-test` hangs (Swift Testing / RupuKit)

A field guide from the 2026-08-27/28 stall investigation. `make macos-test`
(`swift test --package-path apps/rupu-macos/RupuKit`) would reliably go to
0% CPU after ~1100+ green tests — parallel *and* `--no-parallel` — with
`sample` showing every thread idle and nothing blocked in test code.

## Root cause of that incident

Not the toolchain. A test new on `feat/macos-perf-3`
(`pagedSnapshotResetAndRefreshDiscardsALateInFlightLoadMore` in
`Tests/RupuStoreTests/PagedSnapshotTests.swift`) self-deadlocked: its fetch
closure awaited an `AsyncGate` for **every** old-data fetch, but the test's
own setup `refresh()` is an old-data fetch too — so the test task suspended
at the gate before ever reaching its own `gate.open()`. Fixed by gating only
the `loadMore()` fetch (`offset > 0`).

## Why the symptoms were so misleading

- **`sample` shows nothing, all threads idle.** A wedged `await` is a
  suspended Swift Concurrency continuation — it owns no thread. The main
  thread parks in `swift_task_asyncMainDrainQueue`; you will never see test
  code on a stack. 0% CPU + idle threads = "a continuation never resumed",
  not "no evidence".
- **The log's stall point lies.** When stdout is a pipe (piped to `tee`, or
  any non-tty), the test runner block-buffers ~8KB. Several dozen suite/test
  completion lines can be sitting unflushed when the process wedges, so the
  last visible line — often a "suite passed" boundary — is unrelated to the
  actual wedged test. This is why the hang appeared to move between
  suite-transition points across runs.
- **Orphaned `swiftpm-testing-helper` processes** survive a killed
  `swift test` parent and keep holding the `.build` lock, so *subsequent*
  runs queue silently on the lock — which looks like a second, different
  hang. Clean up with: `pkill -9 -f swiftpm-testing-helper`.

## The 3-step recipe that finds the wedged test

1. **Serial + pty.** A tty makes the runner line-buffer, so the log becomes
   truthful; `--no-parallel` makes the first unfinished test *the* wedge:

   ```sh
   script -q /tmp/serial.log swift test \
     --package-path apps/rupu-macos/RupuKit --no-parallel
   ```

   When it stalls, the last `◇ Test X started.` line with no matching
   `✔/✘` is the culprit.

2. **Confirm in isolation.** `swift test --filter <thatTest>` hanging solo
   at 0% CPU proves it's the test, not suite interaction.

3. **If you need live-process proof:** `sample <pid-of-swiftpm-testing-helper>`
   — main thread idle in `swift_task_asyncMainDrainQueue` +
   `mach_msg2_trap` means an un-resumed continuation. Look for the await
   that depends on code later in the same (now suspended) task: gates/
   semaphores opened "later in the test", `async let`s awaited before their
   unblocking side effect, etc.

For parallel-run logs (buffered, interleaved), diff started-vs-finished:

```sh
grep '◇ Test '  run.log | sed -E 's/.*◇ Test ([^ ]+) started.*/\1/'  | sort > /tmp/a
grep -E '✔|✘'   run.log | sed -E 's/.*Test ([^ ]+) (passed|failed).*/\1/' | sort > /tmp/b
comm -23 /tmp/a /tmp/b
```

…but trust it only as a shortlist: buffering means finished tests can appear
unfinished. The serial+pty run is the ground truth.

## Test-authoring rule this bought us

A test helper that suspends (gate, semaphore, continuation) must never sit on
a code path the test's own *setup* traverses. If a fetch/handler closure
gates "the in-flight call being tested", key the gate to what distinguishes
that call (here: `offset > 0`), never to shared state the setup also hits.
The deadlock is deterministic, silent, and — under `swift test` — wedges the
entire run with no failure output, locally and in CI alike.

# Linux isolated Node startup experiment

## Previous observation

Run [44024129940](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/44024129940), head `11b9bf2cf052cf64346ed69096e868b29829159a`:

| Condition | Job | Result |
|---|---:|---|
| Suite first, no runtime pre-run | 133840290122 | 16 isolation tests passed |
| `bwrap --version` first, then suite | 133840289970 | 16 isolation tests passed |
| Delegated cgroup v2 | 133840290139 | 2 tests passed |

The earlier raw job output reported Node probe sequence 2 at 634 ms in the baseline and 440 ms after the `bwrap --version` pre-run; both exited successfully. Sequence 3's 2-second timeout was the intentional `hung-runtime.sh` negative control, not a Node timeout. These timings alone do not establish a cause. `bwrap --version` starts Bubblewrap but does not execute the Node binary or load Node's runtime libraries, so it is not a Node-warm-up treatment.

## Bounded comparison

The manual startup matrix in `.github/workflows/cgroup.yml` creates exactly two fresh Ubuntu 24.04 runners per condition, with `fail-fast: false` so one failed sample does not cancel another:

- `suite-first`: run the original `AIP_TEST_BWRAP=1` `isolation::tests` suite first at Cargo's default test parallelism.
- `isolated-node-first`: call the production `WorkerLaunchConfig::probe(Lang::Node, ...)` path once (`Auto` resolves through the same Linux Bubblewrap `command_spec`), then run the same suite at the same parallelism.

The existing two-second production probe deadline and sandbox mount policy remain in force. Each sample artifact retains the first probe sequence, elapsed time, PID, status and output. Timeout diagnostics traverse only `/proc` process descendants with depth 3, at most 16 processes, at most 8 children per node, and capped per-field reads. The immediate post-kill PID observation is recorded as a snapshot, not interpreted by itself as evidence of a leak. The hung shell fixture is explicitly labelled as a negative control.

No direct Node run, standalone Bubblewrap version run, cache clearing, isolation change, or test parallelism change is part of this treatment. The comparison tests whether one real isolated Node version probe changes the following suite observations; it does not, by itself, prove a general root cause.

## SDK PR failure versus the instrumented comparison

The earlier SDK PR #9 failure is [run 44025732642](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/44025732642), job `133845496357`, head `d83ea820c1f22ca9e6f288fab160b342fbde44d0`. Its Ubuntu 24.04.5 runner (image `20261111.127.1`) ran:

```sh
AIP_TEST_BWRAP=1 cargo test --manifest-path spikes/spike-v4-worker/Cargo.toml --lib isolation::tests --locked --target-dir target
```

Cargo used default test parallelism. The result was 13 passed / 1 failed in 2.00 seconds. `node_extension_imports_local_package_from_node_modules` failed at `WorkerLaunchConfig::probe` with `TimedOut: worker runtime probe exceeded 2 seconds`; the delegated cgroup step was skipped. That job did not set `AIP_TEST_PROBE_DIAGNOSTICS=1` or use `--nocapture`, so its retained log has no probe PID, process state, stdout/stderr byte counts, or Bubblewrap/runtime exit detail. The panic identifies the phase, not why it exceeded the deadline.

The comparison was run at head `c09132858a3dba8595f8e84f1b2d1aa0d85a9e81` in [run 44026018101](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/44026018101). Jobs `133846425749` and `133846425719` were `suite-first` samples 1 and 2; jobs `133846425831` and `133846425750` were `isolated-node-first` samples 1 and 2. Every isolation suite passed 16/16. The suite-first Node version probes took 1195 ms and 135 ms. The isolated-node-first pre-runs took 73 ms and 81 ms; their subsequent suite Node probes took 13 ms and 15 ms. All used Ubuntu 24.04, `AIP_TEST_BWRAP=1`, Cargo's default parallelism, the same production `Auto`/Bubblewrap `node --version` path, and the unchanged two-second deadline. The comparison jobs set `AIP_TEST_PROBE_DIAGNOSTICS=1`, used `--nocapture`, and uploaded their per-runner startup logs as artifacts. The deliberate `hung-runtime.sh` negative control still timed out at about 2 seconds and remains expected. The delegated cgroup job also passed its two tests. Duplicate workflow-dispatch run `44026047737` was cancelled; it is not another sample.

### Systematic-debugging findings

- **Call stack:** `node_extension_imports_local_package_from_node_modules` calls `invoke_worker`; `invoke_worker` first calls `WorkerLaunchConfig::probe(Node)`. The probe resolves `/usr/local/bin/node`, creates the production Bubblewrap command with `--version`, then times out the joined child wait and stdout/stderr drains after two seconds. The failing test never reaches extension import or worker startup.
- **Business meaning:** the timed operation is a runtime version/readiness check. The extension directory is mounted, but Node is not asked to load the extension or its package. This failure alone does not indicate a package-resolution or worker protocol defect.
- **Environment:** the failed probe used the default config (`cgroup_root: None`); it was in the standalone isolation test step before the delegated cgroup step, which was skipped after failure. The latest comparison kept isolation, timeout, and test parallelism fixed. It changed observability (`AIP_TEST_PROBE_DIAGNOSTICS=1`, `--nocapture`, retained artifact) and, for half the samples, added one isolated Node probe before the suite.
- **Logs:** the SDK log records the failing test and two-second `TimedOut`, but not the child state or output. In the comparison, both conditions emitted start/exit records with runtime path, Bubblewrap args, PID, elapsed time, status, and output; the intentional hung fixture emitted a separately classified timeout with a bounded `/proc` sample. No actual Node timeout occurred in the four latest suite samples.

### Falsifiable hypotheses and next discriminator

1. **Runner scheduling or resource contention sometimes delays process startup.** The same suite-first condition measured 1195 ms and 135 ms on fresh runners, consistent with variability but insufficient to assign cause. A future timeout will include process state; the separate startup diagnostics record runner load/CPU pressure during the suite, not atomically at timeout. A repeatable low-load sample near the failure with no runnable/scheduling delay would weaken this hypothesis, but would not alone falsify it.
2. **Bubblewrap startup, Node loading, or pipe draining is the slow phase.** The existing diagnostic distinguishes a still-running child from an exited child whose output pipes have not reached EOF, and records the command and byte counts. A timeout snapshot showing the child already exited with drained pipes would falsify a simple slow-process-start explanation; a live Node process blocked in a repeatable wait channel would narrow it toward the runtime or sandbox path.
3. **Delegated cgroup setup caused this specific failure.** The observed call uses the default config without a cgroup root and failed before the delegated-cgroup step; this hypothesis is contradicted for this event. The later cgroup success does not establish a cgroup fix.

The current comparison workflow already closes the missing-log gap for this isolation suite: it enables probe diagnostics, exposes test output, and preserves the runner log artifact. The latest run produced no real Node failure to explain, so the root cause remains unresolved. Do not change the deadline, add permanent pre-warming, reduce parallelism, or remove the intentional hung-runtime control on this evidence. If the failure recurs, inspect the emitted timeout record and original job artifact before choosing another experiment.

## PR #8 reproduced timeout: suite-first sample 1

The original PR #8 failure was recovered from suite-first sample 1, job `133853290390` (the retained `pr8-suite-first-1.log`, lines 1009–1043). The same test run logged the isolated Python version probes exiting successfully in 5 ms. At the two-second deadline, the isolated Node child was still running at `/usr/local/bin/node --version`; its `/proc` sample reported `State: D (disk sleep)` and `wchan="folio_wait_bit_common"`, with `stdout_bytes_read=0` and `stderr_bytes_read=0`. The probe timed out at 2003 ms and requested a kill. The enclosing Bubblewrap process was waiting for that Node child. The independent `hung-runtime.sh` negative control timed out at 2001 ms as designed.

**Cause assessment:** this is direct evidence that the Node process was blocked in a kernel folio/page wait while starting, before producing version output. A file-backed page-in for Node's executable or one of its mapped runtime files, delayed by a cold page cache or storage I/O, is the best-supported explanation; the fast Python probes in the same job make a general test-suite stall less likely. The wait channel does not identify the exact file or prove that the page was absent from cache rather than being populated or contended by another reader. Thus cold-page-cache delay is a plausible leading cause, not a proven fact. The snapshot does not support a cgroup failure: this probe used `isolation=Auto`/Bubblewrap with no cgroup root, and Bubblewrap itself was waiting on its child.

This observation does not justify relaxing the production startup deadline. The two-second limit remains the maximum initial runtime-readiness wait and still kills and reports a process that exceeds it; it is not an unbounded wait or a retry. The `bwrap` isolation arguments, default test parallelism, and expected two-second `hung-runtime.sh` kill remain unchanged. The PR #8 core clippy repair only combines the adjacent diagnostic-character replacements and does not alter probe behavior.

## Single 5 second probe budget (supersedes the two-second statements above)

Everything above describes the policy at the time of those runs: a two-second probe deadline that was held fixed so the experiment did not hide a failure. That restriction is lifted for this narrow startup policy. `WorkerLaunchConfig::probe` now has one `RUNTIME_PROBE_DEADLINE` of 5 seconds. It is a single wait: no retry, no second attempt, no unbounded wait, and no change to isolation arguments, mounts, parallelism or pre-warming. The operation, attempt and run deadlines of a job, and token, lease and job retries, are unchanged.

Why 5 seconds: the probe is a startup readiness check, run once before the service accepts work, so a longer single wait delays only startup. The two-second value is an implementation choice recorded in `product/INFRASTRUCTURE.md`, not part of any task contract deadline. PR #11's real ORM/PG journey passed, then failed again in cgroup job `133919828803` with the Node probe at 2002 ms in `D` state at `folio_wait` (as reported by the PR #11 owner; this change did not re-read that job). The wait channel shows a file-page wait only; a cold page cache is still unconfirmed. **5 seconds is a proposed operating ceiling.** It is not a measured root cause, and no failure-rate improvement has been demonstrated. The earlier fast samples (13 to 1195 ms) are the only measured start times for this path.

What stays strict:

- `hung-runtime.sh` (a runtime that never prints) still fails at the 5 second limit with `TimedOut`, and `probe` returning an error is what makes server startup refuse (`probe(...)?` in `spike-v6-transport/src/server.rs`).
- Empty output, a non-zero exit and output above the 4 KiB capture limit behave as before; stdout and stderr are still drained and capped at 4 KiB each.

Descendant cleanup: before this change the probe was not a process-group leader and a timeout only signalled the direct child with `start_kill`. A direct local test showed a background child of the runtime still running 3 seconds after the timeout (RED). The probe now starts as its own process group and a timeout calls the existing `reap::kill_tree`. If the leader had already exited and only descendants holding the output pipes remain, `reap::kill_group` signals the leftover group only, because the leader's pid may no longer be safe to signal. A kill request is not proof of reaping: a process in `D` state can outlive it, so the tests observe for a finite 3 seconds and report the result, and the code never waits indefinitely for a reap. The cgroup path (`cgroup.terminate`) is unchanged.

Local evidence (macOS, Isolation::None checked-in fixtures, the existing `target`): with the old two-second limit `runtime_probe_accepts_version_output_later_than_two_seconds` (`delayed-runtime.sh`, version after 3 s) failed with `TimedOut: exceeded 2 seconds`, and both descendant tests failed with the child still running; all pass at 5 seconds. Linux Bubblewrap and cgroup behaviour is checked only by the official Linux CI job output.

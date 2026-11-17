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

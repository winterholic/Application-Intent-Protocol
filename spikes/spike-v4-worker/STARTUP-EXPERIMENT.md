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

The manual workflow `.github/workflows/worker-startup-experiment.yml` creates exactly two fresh Ubuntu 24.04 runners per condition, with `fail-fast: false` so one failed sample does not cancel another:

- `suite-first`: run the original `AIP_TEST_BWRAP=1` `isolation::tests` suite first at Cargo's default test parallelism.
- `isolated-node-first`: call the production `WorkerLaunchConfig::probe(Lang::Node, ...)` path once (`Auto` resolves through the same Linux Bubblewrap `command_spec`), then run the same suite at the same parallelism.

The existing two-second production probe deadline and sandbox mount policy remain in force. Each sample artifact retains the first probe sequence, elapsed time, PID, status and output. Timeout diagnostics traverse only `/proc` process descendants with depth 3, at most 16 processes, at most 8 children per node, and capped per-field reads. The immediate post-kill PID observation is recorded as a snapshot, not interpreted by itself as evidence of a leak. The hung shell fixture is explicitly labelled as a negative control.

No direct Node run, standalone Bubblewrap version run, cache clearing, isolation change, or test parallelism change is part of this treatment. The comparison tests whether one real isolated Node version probe changes the following suite observations; it does not, by itself, prove a general root cause.

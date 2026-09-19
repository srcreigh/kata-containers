# Observe VMM exit independently of stderr completion

[Fix inventory](README.md) · **Lifecycle correction accompanying bounded logging**

Implementation history: `3d88e08bdc91`, `85a851063ac8`.

## Before

Tying child-exit observation to completion of the stderr task confuses two events: the process exiting and the log stream ending. Holding the Firecracker inner lock while awaiting child exit also prevents operations that need the write lock from making progress.

## Change

The wait path polls `try_wait` with short-lived access to the child/inner state, releases the lock between attempts and sleeps 100 ms. It caches the observed exit code. The obsolete exit notification channel was later deleted because no receiver consumed it. Stderr logging/draining does not decide whether the child exited.

## Validation and tradeoffs

The security change and branch-review host-resource record follow the independent paths. Native hypervisor checks and live stop/cleanup exercise ordinary lifecycle; the record does not include every possible inherited-pipe scenario.

The change adds polling latency of up to the interval in ordinary conditions. Existing fallback behavior for an unavailable/reaped child is not full process-recovery proof, and cached/default exit codes should not be interpreted as authenticated guest status.

Validation summaries describe the original development checks, not a fresh test run during publication. Deployment logs and operator scripts are outside this source repository. Source-level regressions, where present, remain alongside the implementation.

## Source

- [src/runtime-rs/crates/hypervisor/src/firecracker/mod.rs](../../src/runtime-rs/crates/hypervisor/src/firecracker/mod.rs)
- [src/runtime-rs/crates/hypervisor/src/firecracker/inner_hypervisor.rs](../../src/runtime-rs/crates/hypervisor/src/firecracker/inner_hypervisor.rs)
- [src/runtime-rs/crates/hypervisor/src/firecracker/inner.rs](../../src/runtime-rs/crates/hypervisor/src/firecracker/inner.rs)

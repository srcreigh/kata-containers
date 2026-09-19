# Enable timers in the runtimes used for network namespace work

[Fix inventory](README.md) · **Regression introduced and repaired in the fork**

Implementation history: `3d88e08bdc91`, `a0e17099d2b4`.

## Before

The new Firecracker API deadline used Tokio timers. Two dedicated current-thread runtimes used for network namespace work had enabled I/O only. Reaching a timeout operation there could panic because no timer driver was installed.

## Change

Both runtime builders use `enable_all()` so I/O and time are available. The API deadline remains enforced rather than being removed to avoid the panic.

## Validation and tradeoffs

The guest-input deployment record identifies the first failed canary and timer correction. The later branch review confirms timer-enabled runtimes at both call sites.

This repairs our deadline integration, not an old upstream timer dependency. It is distinct from 6e2a5b2c2: enabling timers does not prevent an outer async worker from being blocked by a synchronous thread join. That stall has its own detailed causal record.

Validation summaries describe the original development checks, not a fresh test run during publication. Deployment logs and operator scripts are outside this source repository. Source-level regressions, where present, remain alongside the implementation.

## Source

- [src/runtime-rs/crates/runtimes/src/manager.rs](../../src/runtime-rs/crates/runtimes/src/manager.rs)
- [src/runtime-rs/crates/resource/src/manager_inner.rs](../../src/runtime-rs/crates/resource/src/manager_inner.rs)

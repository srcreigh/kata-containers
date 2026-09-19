# Remove the reachable unimplemented Firecracker metrics call

[Fix inventory](README.md) · **Inherited panic-path removal**

Implementation history: `38f5af90b59b`.

## Before

The management metrics path called Firecracker's unimplemented hypervisor-metrics method. Its `todo!()` could panic the shim when a canary scraped metrics, even though no useful Firecracker metrics were being produced.

## Change

The unused VMM metrics forwarding call/path was deleted. Supported shim metrics remain; guest metrics were subsequently separated into their own endpoint. There is no fabricated empty Firecracker metrics implementation pretending to provide measurements.

## Validation and tradeoffs

The host-input deployment record identifies the first failed canary and the follow-up correction before promotion. Subsequent metrics checks passed.

The fix is mostly deletion, but it belongs in this behavioral-fix inventory because it removes a reachable crash. It does not add Firecracker metrics support or prove that all other panic paths are absent.

Validation summaries describe the original development checks, not a fresh test run during publication. Deployment logs and operator scripts are outside this source repository. Source-level regressions, where present, remain alongside the implementation.

## Source

- [src/runtime-rs/crates/runtimes/src/shim_mgmt/handlers.rs](../../src/runtime-rs/crates/runtimes/src/shim_mgmt/handlers.rs)
- [src/runtime-rs/crates/hypervisor/src/firecracker/mod.rs](../../src/runtime-rs/crates/hypervisor/src/firecracker/mod.rs)

# Apply jailer and cgroup restrictions to restored state

[Fix inventory](README.md) · **New fork contract enforcement**

Implementation history: `fe8a41b4e9ff`, `85a851063ac8`.

## Before

Validating fresh configuration alone leaves another input: saved VM/resource state. Removed unjailed, rootless, host-cgroup-overhead or CPU-pinning modes must not silently re-enter through restore paths.

## Change

Firecracker restore requires a jailed state and validates the retained jailer configuration, including seccomp and rootless restrictions. Persistence requires an explicit jail path. Host cgroup restoration checks sandbox-only mode and disabled pinning before constructing its manager. These guards preserve invariants after deletion of alternate branches.

## Validation and tradeoffs

The host-resource/cgroup review ledgers and native contract/persistence suites cover the guards and output-path behavior.

This is not a claim that full shim restart/recovery was repaired or proven. In particular, the review recorded different existing save/read path construction; this work did not resolve that mismatch. Unknown old JSON fields may still be ignored by deserialization.

Validation summaries describe the original development checks, not a fresh test run during publication. Deployment logs and operator scripts are outside this source repository. Source-level regressions, where present, remain alongside the implementation.

## Source

- [src/runtime-rs/crates/hypervisor/src/firecracker/inner.rs](../../src/runtime-rs/crates/hypervisor/src/firecracker/inner.rs)
- [src/runtime-rs/crates/resource/src/cgroups/mod.rs](../../src/runtime-rs/crates/resource/src/cgroups/mod.rs)
- [src/runtime-rs/crates/persist/src/lib.rs](../../src/runtime-rs/crates/persist/src/lib.rs)

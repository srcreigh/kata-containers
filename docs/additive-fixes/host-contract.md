# Reject unsupported runtime settings before VM work

[Fix inventory](README.md) · **New fork contract enforcement**

Implementation history: `3ae371a3ec84`, `85a851063ac8`.

## Before

Deleting implementations while continuing to accept their configuration would make requests appear valid even though the runtime could not honor them. Generic upstream configuration includes many useful features outside this fork's intentionally narrower contract.

## Change

A shared contract validator requires the Firecracker VM runtime, jailer/seccomp, MMIO disks, static VM sizing, tcfilter networking and sandbox-only host cgroups. It rejects templates, shared filesystems, host networking, CPU pinning, dynamic sizing, passfd I/O, initrd boot, encrypted emptyDir and unsupported block tuning, among other removed paths. Configuration adjustment also rejects settings that defaults might otherwise erase. Validation is called before constructing the runtime and again at its boundary. Rootfs selection separately requires exactly one block-backed rootfs (the devmapper path), rejecting shared, overlay, guest-pulled and multilayer alternatives.

## Validation and tradeoffs

Contract/configuration native suites and live negative canaries are recorded in the reduction reports. The source validator is the authoritative full list; this page summarizes the policy rather than duplicating every boolean.

These are intentional restrictions, not upstream bugs. Accepting a config does not prove every workload will succeed. Profiling remains excluded: the Rust shim did not implement the historical Go pprof option.

Validation summaries describe the original development checks, not a fresh test run during publication. Deployment logs and operator scripts are outside this source repository. Source-level regressions, where present, remain alongside the implementation.

## Source

- [src/runtime-rs/crates/runtimes/virt_container/src/contract.rs](../../src/runtime-rs/crates/runtimes/virt_container/src/contract.rs)
- [src/runtime-rs/crates/runtimes/virt_container/src/lib.rs](../../src/runtime-rs/crates/runtimes/virt_container/src/lib.rs)
- [src/runtime-rs/crates/runtimes/src/manager.rs](../../src/runtime-rs/crates/runtimes/src/manager.rs)
- [src/runtime-rs/crates/resource/src/rootfs/mod.rs](../../src/runtime-rs/crates/resource/src/rootfs/mod.rs)

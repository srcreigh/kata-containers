# Fail device attachment before VMM preparation

[Fix inventory](README.md) · **Correction to inherited ineffective queuing**

Implementation history: `85a851063ac8`.

## Before

The Firecracker implementation queued devices received before preparation, but the retained path never drained that pending queue. Returning success in that state could therefore promise attachment without ever configuring the device.

## Change

`FcInner::add_device` now rejects `VmmState::NotReady`. Normal resource preparation must prepare the VMM before attaching network/data devices. The undrained queue and its false-success branch are removed.

## Validation and tradeoffs

The host-resource review traces the ordering and state guard; native hypervisor tests and final canaries exercise the supported prepare/attach sequence.

This was found by source review, not attributed to a measured workload outage. It deliberately rejects an unsupported call order rather than implementing deferred attachment. The independently retained root-drive slot-zero guard still protects the boot disk.

Validation summaries describe the original development checks, not a fresh test run during publication. Deployment logs and operator scripts are outside this source repository. Source-level regressions, where present, remain alongside the implementation.

## Source

- [src/runtime-rs/crates/hypervisor/src/firecracker/inner_device.rs](../../src/runtime-rs/crates/hypervisor/src/firecracker/inner_device.rs)
- [src/runtime-rs/crates/hypervisor/src/firecracker/inner_hypervisor.rs](../../src/runtime-rs/crates/hypervisor/src/firecracker/inner_hypervisor.rs)

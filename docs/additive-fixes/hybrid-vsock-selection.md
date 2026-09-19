# Select Firecracker hybrid-vsock consistently

[Fix inventory](README.md) · **Fork regression repair exposing an upstream capability mismatch**

Implementation history: `29c05b9e903e`, `85a851063ac8`.

## Before

The generic runtime selected its agent transport from hypervisor capabilities. Firecracker did not advertise the hybrid-vsock capability needed by that selection path. Once the native-vsock alternative was removed, this mismatch broke agent connection during the first reduction.

## Change

The initial repair made Firecracker choose/advertise hybrid-vsock. Later simplification removed the generic choice entirely: the host connects to Firecracker's Unix socket and requests the guest vsock port. `prepare_hvsock` remains part of VM preparation. The later deletion of the duplicate vsock resource is separate from this repair and is covered by the network-stall page.

## Validation and tradeoffs

The early deployment observations record the capability mismatch and corrected canary. Current hybrid-vsock tests exercise its handshake; final live pods establish that the retained transport connects.

The original capability method no longer exists at HEAD, so searching only current source misses the historical repair. This does not add native host AF_VSOCK support, and does not explain the later scheduler stall by itself.

Validation summaries describe the original development checks, not a fresh test run during publication. Deployment logs and operator scripts are outside this source repository. Source-level regressions, where present, remain alongside the implementation.

## Source

- [src/runtime-rs/crates/agent/src/sock/mod.rs](../../src/runtime-rs/crates/agent/src/sock/mod.rs)
- [src/runtime-rs/crates/agent/src/sock/hybrid_vsock.rs](../../src/runtime-rs/crates/agent/src/sock/hybrid_vsock.rs)
- [src/runtime-rs/crates/hypervisor/src/firecracker/inner_hypervisor.rs](../../src/runtime-rs/crates/hypervisor/src/firecracker/inner_hypervisor.rs)

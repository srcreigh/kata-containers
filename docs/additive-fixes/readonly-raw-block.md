# Reject raw disks whose read-only intent cannot be honored

[Fix inventory](README.md) · **Safety correction during feature restoration**

Implementation history: `bb812bab2ed0`.

## Before

The restored raw-device path could accept a read-only request even though the pre-created Firecracker drive slots are writable. Updating the drive's backing path does not change that retained slot's read-only setting, so success would misrepresent the requested access restriction.

## Change

Before attachment, the host checks read-only device-cgroup access and block-node read-only state. If the raw disk is read-only, creation fails with an explicit unsupported error. Writable raw devices continue through the MMIO mapping path.

## Validation and tradeoffs

The raw-block canary checks RW functionality and terminal rejection of RO requests without a successful workload start. The harness was corrected to recognize containerd's terminal StartError representation; it must not require a running process to prove rejection.

This restriction is specific to the retained implementation, not a claim that Firecracker can never configure a read-only drive. Filesystem mount read-only handling is a different path. Future RO support would require a drive-pool/configuration change and fresh validation.

Validation summaries describe the original development checks, not a fresh test run during publication. Deployment logs and operator scripts are outside this source repository. Source-level regressions, where present, remain alongside the implementation.

## Source

- [src/runtime-rs/crates/resource/src/manager_inner.rs](../../src/runtime-rs/crates/resource/src/manager_inner.rs)

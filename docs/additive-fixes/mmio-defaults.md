# Default both Firecracker disk drivers to MMIO

[Fix inventory](README.md) · **Fork regression repair and explicit configuration validation**

Implementation history: `d3008471d5cb`, `edae3c45ad23`.

## Before

After alternate transports were deleted, an omitted boot-driver setting could still inherit a generic driver default unsuitable for this Firecracker runtime. Setting the container block driver alone did not set the VM rootfs driver. The first filesystem/network-reduction canary exposed the mismatch.

## Change

Firecracker configuration adjustment defaults both `vm_rootfs_driver` and `block_device_driver` to `virtio-blk-mmio`. The retained contract rejects other drivers. Host device construction uses the host driver name; guest storage uses the normalized `mmioblk` protocol identifier and a concrete guest block path.

## Validation and tradeoffs

The deployment ledger records the failed first attempt and corrected rollout. Configuration/device tests cover defaulting and rejection. A later native test fixture mistakenly used the guest name on the host side; the fixture was corrected without weakening the production check.

This does not add PCI/SCSI compatibility or dynamic device hot-unplug. Explicitly configured unsupported drivers fail instead of being silently converted.

Validation summaries describe the original development checks, not a fresh test run during publication. Deployment logs and operator scripts are outside this source repository. Source-level regressions, where present, remain alongside the implementation.

## Source

- [src/libs/kata-types/src/config/hypervisor/firecracker.rs](../../src/libs/kata-types/src/config/hypervisor/firecracker.rs)
- [src/runtime-rs/crates/resource/src/block_device.rs](../../src/runtime-rs/crates/resource/src/block_device.rs)

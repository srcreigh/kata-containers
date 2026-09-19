# Restore Kubernetes raw block devices with explicit MMIO mappings

[Fix inventory](README.md) · **Restoration plus fork-specific validation**

Implementation history: `0a86d0ebc1bf`, `fe8a41b4e9ff`.

## Before

The early reduction removed raw OCI device handling based on then-current workloads. Kubernetes `volumeDevices` is useful with Firecracker, but restoring it cannot copy host major/minor numbers verbatim: the guest sees different MMIO block devices.

## Change

The host attaches the block device and sends an explicit MMIO guest-path mapping. The guest validates the mapping against an actual block node, updates the OCI device numbers and matching device-cgroup rules, and rejects missing/conflicting mappings. The later simplification keeps only this concrete block path rather than restoring general PCI/VFIO/CDI device machinery.

## Validation and tradeoffs

Device native tests cover mapping/rejection, and the restored-feature and final live reports include writable raw-device canaries. Upstream supplied the underlying device-number translation; the restoration and tighter supported mapping boundary are the fork changes.

This is not a newly invented Firecracker feature. Read-only raw devices are separately rejected because the retained drive pool cannot honor them. Arbitrary GPUs, NIC passthrough and character-device forwarding are not restored.

Validation summaries describe the original development checks, not a fresh test run during publication. Deployment logs and operator scripts are outside this source repository. Source-level regressions, where present, remain alongside the implementation.

## Source

- [src/runtime-rs/crates/resource/src/manager_inner.rs](../../src/runtime-rs/crates/resource/src/manager_inner.rs)
- [src/agent/src/device/mod.rs](../../src/agent/src/device/mod.rs)
- [src/agent/src/storage/mmio.rs](../../src/agent/src/storage/mmio.rs)

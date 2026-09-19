# Keep shared block devices registered until their final user leaves

[Fix inventory](README.md) · **Correction to inherited reference bookkeeping**

Implementation history: `85a851063ac8`.

## Before

The manager removed a device lookup entry after a successful reference decrement even when the device still had users. Another lookup/attachment could then lose the shared identity and make drive-slot bookkeeping inconsistent while the first user remained active.

## Change

Removal now distinguishes a decrement from final release: only the final detach returns a reusable index and removes the manager entry. Remaining references retain both lookup and slot. Checked drive-index arithmetic and reference-count errors remain part of the lifecycle boundary.

## Validation and tradeoffs

The native regression takes two references, removes one, verifies the device is still discoverable, then removes the final reference and checks slot reuse. CSI/multi-volume live checks exercise ordinary sharing but are less specific evidence than the regression.

Firecracker slot release is host bookkeeping; the deleted VMM detach method was a no-op. This does not claim physical device hot-unplug or transactional recovery from every attachment failure.

Validation summaries describe the original development checks, not a fresh test run during publication. Deployment logs and operator scripts are outside this source repository. Source-level regressions, where present, remain alongside the implementation.

## Source

- [src/runtime-rs/crates/hypervisor/src/device/device_manager.rs](../../src/runtime-rs/crates/hypervisor/src/device/device_manager.rs)
- [src/runtime-rs/crates/hypervisor/src/device/driver/virtio_blk_modern.rs](../../src/runtime-rs/crates/hypervisor/src/device/driver/virtio_blk_modern.rs)

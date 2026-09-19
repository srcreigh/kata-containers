# Accept the CSI block-registration format actually supplied by the host

[Fix inventory](README.md) · **Integration compatibility correction**

Implementation history: `bbdca6a60cbc`.

## Before

CSI workspace registrations use `volume-type: blk`, matching the Go-side registration format, while the retained Rust direct-volume path expected `directvol`. Rejecting `blk` broke otherwise ordinary block-backed CSI filesystem volumes.

## Change

The narrowed direct-volume implementation explicitly accepts `blk` and `directvol`, then uses the retained block-backed path. It rejects other backend names. This concerns the registration metadata format: the volume can contain a mounted filesystem and is not necessarily a Kubernetes raw `volumeDevices` node.

## Validation and tradeoffs

The early deployment record distinguishes workspace `blk` from media `directvol` and records successful CSI workloads after the correction.

This does not restore VFIO/SPDK or arbitrary direct-volume backends. Metadata validation and fsGroup propagation are separate changes, documented alongside this one.

Validation summaries describe the original development checks, not a fresh test run during publication. Deployment logs and operator scripts are outside this source repository. Source-level regressions, where present, remain alongside the implementation.

## Source

- [src/runtime-rs/crates/resource/src/volume/direct_volumes/rawblock_volume.rs](../../src/runtime-rs/crates/resource/src/volume/direct_volumes/rawblock_volume.rs)
- [src/runtime-rs/crates/resource/src/volume/direct_volume.rs](../../src/runtime-rs/crates/resource/src/volume/direct_volume.rs)

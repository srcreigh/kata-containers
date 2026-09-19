# Carry CSI ownership intent into the guest

[Fix inventory](README.md) · **Integration correction and new metadata validation**

Implementation history: `3f03616b4b43`.

## Before

A block-backed CSI filesystem could mount successfully but still reject writes from the pod's non-root UID/GID because the Rust path did not forward the explicit ownership metadata. Ignoring other metadata would likewise claim support for behavior that had been removed.

## Change

`supported_metadata` validates a small set: `createFilesystem=false`, an unsigned `fsGroup`, and `Always` or `OnRootMismatch` policy. The constructor places the group and policy in `Storage.fs_group` so the guest's existing ownership logic applies it. Unknown keys, invalid values and requests to create a filesystem are errors.

## Validation and tradeoffs

Early live workspace canaries ran as UID/GID 1000 and wrote through CSI; source/native checks cover accepted policy values and rejection. The existing guest ownership implementation is retained, not newly invented here.

Recursive ownership changes can still be expensive; `OnRootMismatch` has its usual narrower behavior. This does not make filesystem creation available in the minimal image or implement every CSI metadata extension.

Validation summaries describe the original development checks, not a fresh test run during publication. Deployment logs and operator scripts are outside this source repository. Source-level regressions, where present, remain alongside the implementation.

## Source

- [src/runtime-rs/crates/resource/src/volume/direct_volumes/rawblock_volume.rs](../../src/runtime-rs/crates/resource/src/volume/direct_volumes/rawblock_volume.rs)
- [src/agent/src/mount.rs](../../src/agent/src/mount.rs)

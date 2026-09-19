# Choose filesystem versus raw block from the declared type

[Fix inventory](README.md) · **Correction to inherited destination-name heuristic**

Implementation history: `85a851063ac8`.

## Before

A destination beginning with `/dev` was treated as a raw block mount. That misclassified valid filesystem destinations such as `/development` and `/dev/data`; conversely, a raw device need not be mounted under `/dev`.

## Change

`handle_block_volume` follows the declared filesystem type. The caller handling bind mounts of block nodes explicitly requests the raw-bind form and preserves bind flags. CSI and block emptyDir callers supply their actual filesystem type, regardless of the container destination string.

## Validation and tradeoffs

Native volume tests cover filesystem destinations under both misleading prefixes and raw devices at arbitrary destinations. The multi-volume live canary exercises the retained storage paths.

This changes classification, not filesystem detection or formatting. Caller-provided type and metadata still need validation. It does not mean every host path can safely be passed through to a Firecracker guest.

Validation summaries describe the original development checks, not a fresh test run during publication. Deployment logs and operator scripts are outside this source repository. Source-level regressions, where present, remain alongside the implementation.

## Source

- [src/runtime-rs/crates/resource/src/volume/utils.rs](../../src/runtime-rs/crates/resource/src/volume/utils.rs)
- [src/runtime-rs/crates/resource/src/volume/block_volume.rs](../../src/runtime-rs/crates/resource/src/volume/block_volume.rs)

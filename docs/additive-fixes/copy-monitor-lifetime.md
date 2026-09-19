# Give each copied volume its own update monitor

[Fix inventory](README.md) · **Correction to inherited monitor ownership**

Implementation history: `85a851063ac8`.

## Before

The shared source-keyed manager mixed monitor handles, destinations and reference counts. Two containers using the same projected source could overwrite that shared bookkeeping; cleaning up one user could interfere with updates for the other.

## Change

Each `CopyVolume` now owns its own monitor `JoinHandle` and guest destination. Cleanup or Drop aborts only that volume's monitor. Existing inotify watching, debounce, explicit directory copying and Kubernetes projected-volume update handling remain. Source-path equality no longer implies one shared destination or lifetime.

## Validation and tradeoffs

The native volume suite covers monitor ownership. The final multi-volume canary checked updates in two containers, removed the first via CRI, and checked continued updates in the survivor. Its behavioral assertions passed.

There can now be multiple watchers/copies for one source; correctness is preferred over that optimization. The canary cleanup waiter returned exit 1 and cleanup was independently checked, so the record does not claim a perfectly successful end-to-end script exit or identify every restart-watch cause.

Validation summaries describe the original development checks, not a fresh test run during publication. Deployment logs and operator scripts are outside this source repository. Source-level regressions, where present, remain alongside the implementation.

## Source

- [src/runtime-rs/crates/resource/src/volume/copy_volume.rs](../../src/runtime-rs/crates/resource/src/volume/copy_volume.rs)

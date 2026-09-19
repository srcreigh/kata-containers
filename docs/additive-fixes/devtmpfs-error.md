# Propagate failure of the initial device-filesystem mount

[Fix inventory](README.md) · **Correction to inherited error handling**

Implementation history: `85a851063ac8`.

## Before

The early device-filesystem mount could fail without its error reaching the caller. Startup could then continue and fail later when required device nodes were missing, obscuring the original cause.

## Change

`mount_to_rootfs` propagates the initial mount error instead of treating failure as ignorable. The normal successful mount path is unchanged. A regression supplies an invalid filesystem type and requires an error from the helper.

## Validation and tradeoffs

The named `initial_device_mount_failure_is_not_ignored` native test passed in the recorded pass-15 run. Successful live guest startup checks the ordinary path.

This was a source-review correction; the record does not identify a production outage caused by the swallowed error. Error propagation improves diagnosis but does not repair a kernel that lacks a required filesystem.

Validation summaries describe the original development checks, not a fresh test run during publication. Deployment logs and operator scripts are outside this source repository. Source-level regressions, where present, remain alongside the implementation.

## Source

- [src/agent/src/mount.rs](../../src/agent/src/mount.rs)

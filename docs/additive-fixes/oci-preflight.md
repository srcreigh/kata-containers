# Reject removed OCI device and hook features before setup

[Fix inventory](README.md) · **New fork contract enforcement**

Implementation history: `1f9c534fa598`, `9927303def5a`, `749e1521e801`.

## Before

Deleting GPU/CDI, confidential-device and OCI-hook implementations is insufficient if their requests are accepted and partially acted upon. Host setup can otherwise touch devices or start a VM before discovering that the reduced guest cannot satisfy the spec.

## Change

Shared spec validation rejects removed device paths/features, CDI annotations, unsupported accelerator-selection environment values, sealed-secret inputs and nonempty OCI lifecycle hooks. Host task/container boundaries and guest preflight use these checks before the corresponding device/storage work. Ordinary supported block devices still follow their explicit MMIO path.

## Validation and tradeoffs

Shared-types and RPC native tests cover rejection; historical live negative probes verify selected requests fail. The branch ledgers identify the call sites, not just the helper definition.

These rules express the fork's supported spec, not a universal Kubernetes policy. OCI hooks are different from Kubernetes exec/HTTP lifecycle actions. The guards are not a general sanitizer for every possible environment variable or arbitrary device naming convention.

Validation summaries describe the original development checks, not a fresh test run during publication. Deployment logs and operator scripts are outside this source repository. Source-level regressions, where present, remain alongside the implementation.

## Source

- [src/libs/kata-types/src/device.rs](../../src/libs/kata-types/src/device.rs)
- [src/runtime-rs/crates/runtimes/src/manager.rs](../../src/runtime-rs/crates/runtimes/src/manager.rs)
- [src/agent/src/rpc.rs](../../src/agent/src/rpc.rs)

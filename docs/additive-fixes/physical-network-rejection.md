# Reject physical network passthrough before host rebinding

[Fix inventory](README.md) · **New fork safety boundary**

Implementation history: `42a974484f65`, `85a851063ac8`.

## Before

The generic network path could recognize a physical interface and proceed toward PCI/VFIO rebinding. This fork has no supported physical-NIC passthrough implementation, so allowing that setup could alter host networking for a feature that cannot complete.

## Change

Physical-interface detection now returns an explicit unsupported error instead of entering the rebinding path. Directly attachable networking configuration is also rejected before resource setup. Supported CNI interfaces continue through TAP/tcfilter; unsupported endpoint types remain errors.

## Validation and tradeoffs

The device/network reduction record and host-resource review trace the rejection before rebinding. Native network tests cover retained endpoint handling and early rejection; ordinary CNI networking passed live canaries.

This prevents host mutation for a removed feature. It does not disable Kubernetes networking generally or claim that every future Firecracker network feature is impossible.

Validation summaries describe the original development checks, not a fresh test run during publication. Deployment logs and operator scripts are outside this source repository. Source-level regressions, where present, remain alongside the implementation.

## Source

- [src/runtime-rs/crates/resource/src/network/network_with_netns.rs](../../src/runtime-rs/crates/resource/src/network/network_with_netns.rs)
- [src/runtime-rs/crates/resource/src/network/mod.rs](../../src/runtime-rs/crates/resource/src/network/mod.rs)

# Restore useful API errors with bounded bodies and deadlines

[Fix inventory](README.md) · **Restoration plus host input hardening**

Implementation history: `0a86d0ebc1bf`, `bb812bab2ed0`, `3d88e08bdc91`.

## Before

The reduction briefly discarded Firecracker's error body, making failures harder to diagnose. Restoring unrestricted body collection would reintroduce an input/memory boundary problem, and repeated request/body waits also needed a total time bound.

## Change

Non-success responses retain HTTP status and bounded lossy-UTF8 error text, including the useful final failure through retries. Error-body accumulation stops at 64 KiB. A single ten-second deadline covers the request/retry/body operation, with the existing retry structure bounded by that deadline.

## Validation and tradeoffs

Native Firecracker API regressions cover oversized error bodies and preservation of error text through retries; the deadline was checked by source inspection, not a dedicated expiry test; the security and deployment records trace the restored diagnostics. This timeout addition also caused the missing-timer regression documented separately.

The peer is the VMM API, so this is a different boundary from direct guest RPC replies. Error text remains untrusted diagnostic text. Oversized bodies produce a limit error rather than a truncated original message; a deadline expiry reports the deadline error rather than the last API body. A timer only works while its Tokio runtime can make progress; the later network-thread stall required a separate scheduler fix.

Validation summaries describe the original development checks, not a fresh test run during publication. Deployment logs and operator scripts are outside this source repository. Source-level regressions, where present, remain alongside the implementation.

## Source

- [src/runtime-rs/crates/hypervisor/src/firecracker/fc_api.rs](../../src/runtime-rs/crates/hypervisor/src/firecracker/fc_api.rs)

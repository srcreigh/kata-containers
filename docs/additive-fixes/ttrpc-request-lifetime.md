# Bound outstanding RPC work and clean up cancelled unary calls

[Fix inventory](README.md) · **Patch to pinned upstream ttrpc 0.9.0**

Implementation history: `3d88e08bdc91`.

## Before

A timed-out or cancelled unary request could leave registration state behind. Sending into a congested outgoing queue was outside the old response-wait timeout. A noncooperating guest could therefore retain host work beyond the intended request lifetime.

## Change

A drop guard removes unary registration on completion, failure, timeout or cancellation. At most 256 registrations may exist, and allocation rejects an already-present stream ID. Unary reply queues hold one item. For a nonzero timeout, the deadline covers enqueue plus waiting for a response; intentionally timeout-free long polls remain possible.

## Validation and tradeoffs

Vendored regressions exercise timeout/cancellation cleanup and the pending-call cap. The native ttrpc suite passed; KATA-PATCHES records the original upstream version/commit for maintenance.

These are per-client bounds, not a global memory budget. Exhaustion now fails requests, which can affect availability under legitimate extreme concurrency. The RAII claim is specifically about the unary path; the retained generic streaming API has a different lifecycle.

Validation summaries describe the original development checks, not a fresh test run during publication. Deployment logs and operator scripts are outside this source repository. Source-level regressions, where present, remain alongside the implementation.

## Source

- [vendor/ttrpc/src/asynchronous/client.rs](../../vendor/ttrpc/src/asynchronous/client.rs)
- [vendor/ttrpc/KATA-PATCHES.md](../../vendor/ttrpc/KATA-PATCHES.md)

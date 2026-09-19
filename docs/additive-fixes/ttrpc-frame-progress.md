# Bound frame progress and dispatch work from the peer

[Fix inventory](README.md) · **Patch to pinned upstream ttrpc 0.9.0**

Implementation history: `3d88e08bdc91`.

## Before

The old async reader drained an oversized peer-selected body before reporting failure. A partial frame could wait indefinitely. Response dispatch spawned tasks whose queue waits could accumulate independently of useful callers.

## Change

Once the first byte arrives, the entire remaining header/body has a 30-second deadline. Oversized frames fail immediately without draining their bodies; the existing 4 MiB maximum is retained. Unary responses must have the correct message type and zero flags. Client dispatch uses bounded nonblocking delivery instead of spawning a task per response; malformed framing terminates the connection.

## Validation and tradeoffs

Tests cover oversized and malformed replies, idle versus partial-frame behavior, and continued ordinary RPCs where appropriate. The pinned-package diff separates these patches from copied upstream library code.

Idle connections deliberately have no frame-progress timer until a frame begins. This does not apply the same implementation to every synchronous transport path. The 4 MiB cap was not newly invented here, and a frame failure can terminate unrelated pending calls on that connection.

Validation summaries describe the original development checks, not a fresh test run during publication. Deployment logs and operator scripts are outside this source repository. Source-level regressions, where present, remain alongside the implementation.

## Source

- [vendor/ttrpc/src/proto.rs](../../vendor/ttrpc/src/proto.rs)
- [vendor/ttrpc/src/asynchronous/client.rs](../../vendor/ttrpc/src/asynchronous/client.rs)
- [vendor/ttrpc/src/asynchronous/connection.rs](../../vendor/ttrpc/src/asynchronous/connection.rs)

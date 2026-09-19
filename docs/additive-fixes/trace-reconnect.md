# Discard every failed guest trace connection before the next batch

[Fix inventory](README.md) · **Correction to inherited exporter error handling**

Implementation history: `85a851063ac8`.

## Before

The guest exporter cleared a failed connection only for NotConnected. BrokenPipe or ConnectionReset could therefore leave a dead stream cached and make subsequent batches keep using it. A connection-result `.map(...)` also logged the error message on success.

## Change

Any write/export error now removes the cached connection. The next batch reconnects through the normal connection path. Connection-error logging uses `map_err`, so successful connection no longer emits the misleading error. Eight-byte framing and complete payload writes are retained.

## Validation and tradeoffs

The branch review identifies both corrections, and the integrated guest build/traced canary establishes the ordinary export path. The recorded live check is not a forced-disconnection/reconnect experiment.

The failed batch can be lost; this is reconnection for later work, not exactly-once delivery or automatic replay. `write_all` already existed upstream, so it is not credited as a new partial-write fix.

Validation summaries describe the original development checks, not a fresh test run during publication. Deployment logs and operator scripts are outside this source repository. Source-level regressions, where present, remain alongside the implementation.

## Source

- [src/agent/vsock-exporter/src/lib.rs](../../src/agent/vsock-exporter/src/lib.rs)

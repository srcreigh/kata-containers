# Stop unbounded VMM logging without closing its pipe

[Fix inventory](README.md) · **Host diagnostics hardening**

Implementation history: `3d88e08bdc91`.

## Before

Firecracker stderr is another host input stream. A newline-free or very high-rate stream can grow buffers/flood logs. Simply abandoning and closing the read end after a limit violation can disrupt the VMM's writes through pipe errors.

## Change

The reader caps records at 64 KiB and logging at 1024 records/1 MiB per second. On logging failure/limit violation it stops interpreting records but continues reading into an 8 KiB discard buffer, with a small delay, until EOF. Child exit is observed separately.

## Validation and tradeoffs

Shared reader/rate-limit tests exercise bounded input handling; the host-resource review traces the drain path and retained child lifecycle. Ordinary VMM startup/stop passed live validation.

Draining is throttled and can backpressure the VMM; this does not guarantee a malicious/noisy VMM remains fully available. Further stderr diagnostics are intentionally lost after violation. It is not a global log-storage quota.

Validation summaries describe the original development checks, not a fresh test run during publication. Deployment logs and operator scripts are outside this source repository. Source-level regressions, where present, remain alongside the implementation.

## Source

- [src/runtime-rs/crates/hypervisor/src/firecracker/inner.rs](../../src/runtime-rs/crates/hypervisor/src/firecracker/inner.rs)
- [src/libs/kata-sys-util/src/guest_io.rs](../../src/libs/kata-sys-util/src/guest_io.rs)

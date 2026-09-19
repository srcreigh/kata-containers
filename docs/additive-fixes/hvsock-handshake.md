# Read an exact, bounded Firecracker connection acknowledgement

[Fix inventory](README.md) · **Host transport hardening**

Implementation history: `3d88e08bdc91`.

## Before

A loose acknowledgement match can accept malformed success-looking text. A buffered line reader can also consume bytes following the newline; discarding that reader then loses the start of the RPC stream when ACK and payload arrive together.

## Change

The connection helper reads through the newline without read-ahead, enforces a 32-byte acknowledgement bound, and requires the exact supported OK/decimal-port shape. The dial timeout covers connection plus handshake, within the configured retry policy. The returned UnixStream retains any subsequent RPC bytes.

## Validation and tradeoffs

The regression coalesces acknowledgement and following payload and verifies preservation, then checks rejection of substring/malformed responses. Native agent tests and live VM connections passed.

This parses a Firecracker transport acknowledgement, not guest identity authentication. The returned decimal port is syntax-checked, not a cryptographic assertion. Slow or malformed peers now cause bounded connection failures and retries.

Validation summaries describe the original development checks, not a fresh test run during publication. Deployment logs and operator scripts are outside this source repository. Source-level regressions, where present, remain alongside the implementation.

## Source

- [src/runtime-rs/crates/agent/src/sock/hybrid_vsock.rs](../../src/runtime-rs/crates/agent/src/sock/hybrid_vsock.rs)
- [src/runtime-rs/crates/agent/src/sock/mod.rs](../../src/runtime-rs/crates/agent/src/sock/mod.rs)

# Stop decoding RPC bodies whose only consumer needs success or failure

[Fix inventory](README.md) · **Input-surface correction primarily achieved by deletion**

Implementation history: `61e6c3413994`, `6f7b87918f49`, `749e1521e801`.

## Before

The host decoded responses and converted network/device structures even where callers consumed only success or failure. Those unused conversions added guest-controlled parsing and included fallible enum assumptions. Their output did not configure host networking or otherwise affect the successful call.

## Change

The host serializes requests and uses the normal ttrpc envelope/status machinery. For acknowledgement-only methods it discards the method payload after bounds checks. It retains typed decoding for nine consumed response kinds. UpdateInterface/UpdateRoutes request behavior and error propagation remain; their unused response converters and guest reply construction are removed.

## Validation and tradeoffs

The host-agent reduction report traces each consumer. Native tests cover ignoring unused payload data while preserving RPC errors; live network and lifecycle canaries demonstrate normal operation.

This is not a handwritten replacement protobuf framing/status parser. An OK envelope with malformed unused method bytes can intentionally succeed, because those bytes have no supported meaning to the caller. Framing, status and needed payloads are still parsed and require their own controls.

Validation summaries describe the original development checks, not a fresh test run during publication. Deployment logs and operator scripts are outside this source repository. Source-level regressions, where present, remain alongside the implementation.

## Source

- [src/runtime-rs/crates/agent/src/kata/mod.rs](../../src/runtime-rs/crates/agent/src/kata/mod.rs)
- [src/runtime-rs/crates/agent/src/kata/agent.rs](../../src/runtime-rs/crates/agent/src/kata/agent.rs)

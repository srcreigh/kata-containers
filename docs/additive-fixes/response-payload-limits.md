# Limit consumed RPC payloads and selected collections

[Fix inventory](README.md) · **New host input hardening**

Implementation history: `3d88e08bdc91`, `749e1521e801`.

## Before

A transport-wide ceiling does not express how much data a particular method should return. Small acknowledgements, process statuses and event IDs do not need the same allowance as statistics or metrics.

## Change

After ttrpc envelope decoding and before method-body decoding, the host applies 4 KiB limits to WaitProcess/WriteStdin/GetOOMEvent; 64 KiB to stream reads, diagnostics, volume stats and default acknowledgements; and 1 MiB to container stats/metrics. Post-decode validation caps memory statistics at 4096 keys, hugepage entries at 256, block-service entries at 4096, volume usage entries at 16, and OOM IDs at 1–256 bytes.

## Validation and tradeoffs

Native host-agent tests cover excessive collections and event IDs; the guest-input report records the payload policy and live ordinary input checks.

Collection limits are checked after protobuf allocation. The envelope is still parsed under the transport ceiling, so these are not pre-allocation bounds for every layer. Valid-size values can still be false, and not every numeric/string field receives semantic validation.

Validation summaries describe the original development checks, not a fresh test run during publication. Deployment logs and operator scripts are outside this source repository. Source-level regressions, where present, remain alongside the implementation.

## Source

- [src/runtime-rs/crates/agent/src/kata/mod.rs](../../src/runtime-rs/crates/agent/src/kata/mod.rs)
- [src/runtime-rs/crates/agent/src/kata/response.rs](../../src/runtime-rs/crates/agent/src/kata/response.rs)

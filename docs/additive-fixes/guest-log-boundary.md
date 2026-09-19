# Bound guest log records and preserve their provenance

[Fix inventory](README.md) · **Host input hardening and parser removal**

Implementation history: `61e6c3413994`, `3d88e08bdc91`.

## Before

Parsing guest log JSON as host structured logging let guest-supplied fields influence severity/metadata. Unbounded lines and rapid records could also allocate memory or flood host logging.

## Change

The forwarder emits guest text with a fixed guest origin and host-selected INFO treatment rather than interpreting its JSON structure. A record is bounded to 16 KiB before unchecked growth, with 1024-record/1 MiB per-second budgets. Invalid or excessive input ends that logging stream; agent RPC transport is separate.

## Validation and tradeoffs

Shared bounded-reader/rate tests and the guest-input audit explain the limits. Live input checks cover ordinary logs, not an exhaustive hostile-VM logging campaign.

This sacrifices further guest logs after a violation and does not sanitize every control character. Per-stream rate limits are not a total journald/disk quota. Downstream collectors must preserve provenance and must not re-interpret guest JSON as trusted host fields.

Validation summaries describe the original development checks, not a fresh test run during publication. Deployment logs and operator scripts are outside this source repository. Source-level regressions, where present, remain alongside the implementation.

## Source

- [src/runtime-rs/crates/agent/src/log_forwarder.rs](../../src/runtime-rs/crates/agent/src/log_forwarder.rs)
- [src/libs/kata-sys-util/src/guest_io.rs](../../src/libs/kata-sys-util/src/guest_io.rs)

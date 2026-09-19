# Require containerd event publishing at startup

[Fix inventory](README.md) · **New fork integration invariant**

Implementation history: `61e6c3413994`.

## Before

The service could fall back to logging events when `TTRPC_ADDRESS` was missing. For this containerd-only Kubernetes runtime, that is not equivalent to delivering lifecycle events to the supervisor: the runtime could appear to operate while its consumer never received them.

## Change

Publisher construction now requires a present, nonempty containerd TTRPC address and creates the actual forwarder. The log-only alternative is removed. Namespace and event encoding remain those of the existing containerd integration.

## Validation and tradeoffs

The host-input reduction report describes the removed fallback; source inspection shows failure occurs at publisher construction. Live Kubernetes lifecycle checks exercise the configured publisher path.

This turns a misconfigured launch into an early error. It does not guarantee delivery after startup: forwarding can still fail and be logged, and this change does not introduce durable queues or retries.

Validation summaries describe the original development checks, not a fresh test run during publication. Deployment logs and operator scripts are outside this source repository. Source-level regressions, where present, remain alongside the implementation.

## Source

- [src/runtime-rs/crates/service/src/event.rs](../../src/runtime-rs/crates/service/src/event.rs)
- [src/runtime-rs/crates/service/src/manager.rs](../../src/runtime-rs/crates/service/src/manager.rs)

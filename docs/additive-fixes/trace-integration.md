# Restore linked tracing and a compatible host exporter

[Fix inventory](README.md) · **Restoration plus integration corrections**

Implementation history: `0a86d0ebc1bf`, `bb812bab2ed0`.

## Before

Tracing had been removed, although it is useful for Kubernetes/Firecracker diagnosis. Reintroducing crates alone was insufficient: RPC context had to cross the boundary, and the Jaeger exporter expected a Hyper 0.14 client while the main runtime uses Hyper 1.

## Change

The host injects the current trace context into ttrpc metadata; guest handlers extract it and attach it as parent context. Runtime tracing sets the supported propagator and uses the exporter's compatible HTTP-client integration. Guest spans travel through the restored vsock exporter/Firecracker forwarder. Instrumentation uses `skip_all` at relevant boundaries to avoid automatically recording entire request arguments.

## Validation and tradeoffs

The restoration record describes the Hyper integration correction. The final traced canary recorded seven batches and 44 linked spans, demonstrating actual exported context rather than merely successful compilation.

Tracing is opt-in and profiling was intentionally left out. `skip_all` does not guarantee that no explicit log/span field contains sensitive data. Guest span contents remain untrusted; input bounds and provenance are separate fixes.

Validation summaries describe the original development checks, not a fresh test run during publication. Deployment logs and operator scripts are outside this source repository. Source-level regressions, where present, remain alongside the implementation.

## Source

- [src/runtime-rs/crates/runtimes/src/tracer.rs](../../src/runtime-rs/crates/runtimes/src/tracer.rs)
- [src/runtime-rs/crates/agent/src/kata/mod.rs](../../src/runtime-rs/crates/agent/src/kata/mod.rs)
- [src/agent/src/tracer.rs](../../src/agent/src/tracer.rs)
- [src/agent/src/rpc.rs](../../src/agent/src/rpc.rs)

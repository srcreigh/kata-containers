# Firecracker distributed tracing

Tracing defaults off. Enable both sides for a Kubernetes pod:

```yaml
metadata:
  annotations:
    io.katacontainers.config.runtime.enable_tracing: "true"
    io.katacontainers.config.agent.enable_tracing: "true"
```

Alternatively set runtime.enable_tracing and agent.kata.enable_tracing in the
runtime TOML. The host emits W3C trace context on agent RPCs; the guest spans
use it as their parent. Guest instrumentation skips argument values.

The existing host exporter sends Jaeger Thrift over HTTP to
runtime.jaeger_endpoint (default http://localhost:14268/api/traces).
The agent exports JSON-encoded spans through guest vsock port 10240. For each
traced VM, run the bundled forwarder on its worker:

```
sudo /path/to/kata-trace-forwarder \
  --socket-path /run/kata/firecracker/SANDBOX_ID/root/kata.hvsock \
  --jaeger-endpoint 127.0.0.1:6831
```

Both destinations require a collector with Jaeger ingestion enabled; these are
not native OTLP endpoints. The forwarder binds the _10240 sibling socket,
drops root groups/uid/gid to nobody, and exports Jaeger UDP. It is an explicit
operator process, not a service started automatically by the shim. Start it as
soon as the VM socket directory exists; spans emitted before a listener exists
can be lost. Stop it when the VM is removed. An existing socket is never unlinked
automatically. Runtime endpoint credentials are configured in host TOML, not
pod annotations.

The forwarder matches the historical guest JSON/OpenTelemetry format. It rejects
malformed/truncated spans and frames above 4 MiB, and limits idle reads. The
guest drops a failed connection so later batches can reconnect. Enabling guest
tracing deliberately restores this optional host parser; ordinary workloads
with tracing disabled do not emit these frames.

Profiling is excluded: enable_pprof never implemented a profiler in this Rust
shim. Logs, metrics and the existing management endpoints remain available.

Guest spans are untrusted. The forwarder caps frames at 4 MiB, requires completion
within five seconds of the first byte, and accepts at most 256 frames / 8 MiB per
second across connections. Span attributes, events and links are limited to 256
entries each, with the same attribute limit on each event/link. Names are limited
to 4 KiB and status messages to 16 KiB. Jaeger process metadata includes the
host-controlled `kata.telemetry.origin=guest` tag. Keep this service distinct from
host tracing; guest trace IDs, relationships and measurements are not attested.

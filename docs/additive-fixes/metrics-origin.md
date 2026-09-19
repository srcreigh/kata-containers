# Separate host metrics from guest metrics

[Fix inventory](README.md) · **Telemetry provenance correction**

Implementation history: `3d88e08bdc91`.

## Before

Combining host and guest Prometheus text in one endpoint obscures the trust boundary. A guest can supply metric names, labels and values that look like host observations.

## Change

`/metrics` returns host shim metrics; `/metrics/guest` returns bounded guest metrics. Responses carry `X-Kata-Metrics-Origin` with the selected origin. The response helper caps text at 1 MiB. The split avoids giving guest text the implicit identity of the host scrape.

## Validation and tradeoffs

Native management-handler tests and live input canaries check the endpoints/provenance. The security audit follows the data into downstream collection.

Collectors must scrape the new guest endpoint deliberately and preserve its origin label; the header is not automatically a Prometheus label. Guest text is still guest-controlled Prometheus input. The change does not validate every metric name or make guest measurements authoritative.

Validation summaries describe the original development checks, not a fresh test run during publication. Deployment logs and operator scripts are outside this source repository. Source-level regressions, where present, remain alongside the implementation.

## Source

- [src/runtime-rs/crates/runtimes/src/shim_mgmt/handlers.rs](../../src/runtime-rs/crates/runtimes/src/shim_mgmt/handlers.rs)

# Bound guest trace frames and assign host-controlled exporter identity

[Fix inventory](README.md) · **Host telemetry hardening**

Implementation history: `3d88e08bdc91`.

## Before

A guest-controlled length prefix can trigger a large allocation; trickled frames can occupy the synchronous forwarder. Decoded span collections and exporter identity are additional boundaries before data leaves Kata for the tracing system.

## Change

Frames must be nonempty and at most 4 MiB, with a five-second completion deadline after the first read and a 30-second initial-read timeout. A shared rate budget permits 256 frames/8 MiB per second. After JSON decoding, names/status and attributes/events/links receive explicit size/count checks. Exporter service/resource configuration comes from the host, including `kata.telemetry.origin=guest`.

## Validation and tradeoffs

Native tests cover invalid/truncated/oversized frames, expired deadlines and the long-path listener. Historical live tracing verifies normal export; the guest-input report records the trust boundary.

Collection limits follow JSON allocation. Guest span IDs, relationships, tags and claims remain untrusted, and a blocked downstream exporter is a separate concern from the input read deadline. Collector queries must preserve the host-assigned provenance instead of treating guest data as host measurements.

Validation summaries describe the original development checks, not a fresh test run during publication. Deployment logs and operator scripts are outside this source repository. Source-level regressions, where present, remain alongside the implementation.

## Source

- [src/tools/trace-forwarder/src/main.rs](../../src/tools/trace-forwarder/src/main.rs)
- [src/libs/kata-sys-util/src/guest_io.rs](../../src/libs/kata-sys-util/src/guest_io.rs)

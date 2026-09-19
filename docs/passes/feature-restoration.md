# Restore useful Kubernetes/Firecracker features

[Documentation history](../../README.md#documentation-history)

Original documentation record: **2026-09-19 01:08:59 UTC**.
Public implementation reference(s): `0a86d0ebc–bb812bab2`.

A review of earlier workload-specific deletions identified useful Firecracker features to restore: Kubernetes raw block `volumeDevices`, opt-in distributed tracing and detailed VMM API errors. Profiling remained excluded because the Rust shim lacked the historical Go pprof implementation.

The restoration translated block identities through explicit MMIO mappings and rejected read-only raw disks the retained drive pool could not enforce. Tracing follow-ups corrected exporter-library compatibility and long Unix-socket binding paths. The original record included writable raw-device checks, explicit read-only rejection and linked host/guest traces. These were restorations and integration corrections, not newly invented VMM capabilities.

## About this record

This is a sanitized retrospective summary of the original pass documentation.
The timestamp is its original Git author timestamp, converted to UTC; it is not
the creation time of this public summary or an exact test/deployment time.
Historical validation claims have not been rerun for this documentation update.
For individual behavioral corrections, see the [additive-fix summary](../additive-fixes/README.md).

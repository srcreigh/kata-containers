# MMIO-only disk transport and swap-worker reduction

[Documentation history](../../README.md#documentation-history)

Original documentation record: **2026-09-18 19:05:01 UTC**.
Public implementation reference(s): `edae3c45a`.

Alternate disk transports and the host guest-swap worker were removed. The remaining host configuration, device construction, guest storage identifiers and boot parameters were aligned around Firecracker MMIO block devices.

The original record reported storage/workload checks after the transport cleanup. Removing the worker that provisions guest swap is separate from retaining correct per-container cgroup memory/swap accounting and limit conversion. Later resource-validation work made that distinction explicit.

## About this record

This is a sanitized retrospective summary of the original pass documentation.
The timestamp is its original Git author timestamp, converted to UTC; it is not
the creation time of this public summary or an exact test/deployment time.
Historical validation claims have not been rerun for this documentation update.
For individual behavioral corrections, see the [additive-fix summary](../additive-fixes/README.md).

# Guest device and service reduction

[Documentation history](../../README.md#documentation-history)

Original documentation record: **2026-09-18 16:14:21 UTC**.
Public implementation reference(s): `1f9c534fa–42a974484`.

Guest CDI/GPU, PCI and memory-agent subsystems were removed. Shared validation rejects device-selection metadata and unsupported device paths before setup. Physical network passthrough was rejected before reaching host-driver rebinding.

This reduced whole unused subsystems rather than only their RPC entry points. Ordinary block-backed storage and CNI networking remained necessary. The original validation record reported successful workload checks and negative device-feature probes. Raw block-device support was revisited and restored later; that later decision must not be read back into this pass's narrower initial assumptions.

## About this record

This is a sanitized retrospective summary of the original pass documentation.
The timestamp is its original Git author timestamp, converted to UTC; it is not
the creation time of this public summary or an exact test/deployment time.
Historical validation claims have not been rerun for this documentation update.
For individual behavioral corrections, see the [additive-fix summary](../additive-fixes/README.md).

# Initial Firecracker-only reduction

[Documentation history](../../README.md#documentation-history)

Original documentation record: **2026-09-18 14:49:17 UTC**.
Public implementation reference(s): `3ae371a3e–3f03616b4`.

The first pass removed the Go runtime and alternate VMM/runtime implementations, narrowed container roots and storage to the retained block-backed path, and introduced an explicit unsupported-feature contract. The supported target became Linux x86-64 with jailed Firecracker, static VM sizing and a PID-1 guest agent.

Integration work corrected the minimal guest filesystem layout, selected hybrid-vsock consistently, accepted the CSI `blk` registration format, and forwarded explicit filesystem ownership metadata. These changes established the working baseline for later reductions; they were not evidence that every upstream feature was unnecessary.

The original record reported boot, networking, exec, projected-file and CSI workload checks. Deployment inventories and operator commands are not part of this public summary.

## About this record

This is a sanitized retrospective summary of the original pass documentation.
The timestamp is its original Git author timestamp, converted to UTC; it is not
the creation time of this public summary or an exact test/deployment time.
Historical validation claims have not been rerun for this documentation update.
For individual behavioral corrections, see the [additive-fix summary](../additive-fixes/README.md).

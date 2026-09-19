# Pass 12: PID-1 guest and cgroup-v2 reduction

[Documentation history](../../README.md#documentation-history)

Original documentation record: **2026-09-19 01:51:23 UTC**.
Public implementation reference(s): `749e1521e`.

This pass removed service-manager/non-PID-1 guest paths, guest systemd/D-Bus cgroup handling and cgroup-v1 alternatives. Resource validation rejects unsupported v1-only controls before mutations. Retained lifecycle, exec, namespace isolation, cgroup-v2 accounting and hugepage controls continued to serve the supported contract.

The pass further narrowed unused RPC results and generated protocol code. It also corrected the resource setter's zero-swap-limit case: a positive combined OCI memory+swap limit can convert to a zero cgroup-v2 swap allowance and still needs to be written.

The original record reported native and workload validation. Conversion tests are not a live demonstration of a nonzero-to-zero swap transition, and selected negative RPC probes are not proof that every unsupported request is covered.

## About this record

This is a sanitized retrospective summary of the original pass documentation.
The timestamp is its original Git author timestamp, converted to UTC; it is not
the creation time of this public summary or an exact test/deployment time.
Historical validation claims have not been rerun for this documentation update.
For individual behavioral corrections, see the [additive-fix summary](../additive-fixes/README.md).

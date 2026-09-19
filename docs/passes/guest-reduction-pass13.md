# Pass 13: unused runtime and guest machinery

[Documentation history](../../README.md#documentation-history)

Original documentation record: **2026-09-19 02:23:31 UTC**.
Public implementation reference(s): `fe8a41b4e–8036e0730`.

Unused utility dependencies, duplicate device/network machinery and dynamic-VM sizing code were removed. The retained MAC serialization replaced a much larger utility dependency. Storage dispatch and MMIO mapping were simplified without discarding useful Kubernetes raw block, hugepages, tracing or guest filesystem mounts.

Restored-state guards enforce the reduced jailer/cgroup contract, and empty unprepared VM paths cannot become cleanup targets. The vendored transport rejected removed schemes before socket creation. Test-only compilation cleanup followed the production reduction.

The documentation recorded native checks and workload validation, including a bounded resource-update/restore exercise. Saved-state validation is not proof of complete shim recovery; deployment tooling and its machine-specific records are maintained separately.

## About this record

This is a sanitized retrospective summary of the original pass documentation.
The timestamp is its original Git author timestamp, converted to UTC; it is not
the creation time of this public summary or an exact test/deployment time.
Historical validation claims have not been rerun for this documentation update.
For individual behavioral corrections, see the [additive-fix summary](../additive-fixes/README.md).

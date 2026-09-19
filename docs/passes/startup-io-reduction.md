# Guest startup and I/O reduction

[Documentation history](../../README.md#documentation-history)

Original documentation record: **2026-09-18 17:00:21 UTC**.
Public implementation reference(s): `a3385f1e1–5808dd87e`.

Unused guest services and the alternate pass-fd I/O path were removed. The guest configuration parser became stricter so removed service options would fail explicitly, while ordinary agent RPC streams continued carrying container I/O.

The first candidate exposed a producer/consumer mismatch: host defaults still generated options for services the guest no longer implemented. The follow-up removed that emission rather than weakening guest validation. The corrected record reported successful startup and workload checks. This was a regression introduced by narrowing the guest contract, not an inherited upstream boot failure.

## About this record

This is a sanitized retrospective summary of the original pass documentation.
The timestamp is its original Git author timestamp, converted to UTC; it is not
the creation time of this public summary or an exact test/deployment time.
Historical validation claims have not been rerun for this documentation update.
For individual behavioral corrections, see the [additive-fix summary](../additive-fixes/README.md).

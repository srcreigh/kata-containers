# Guest RPC surface reduction

[Documentation history](../../README.md#documentation-history)

Original documentation record: **2026-09-18 15:29:20 UTC**.
Public implementation reference(s): `c31d596f9`.

This pass removed seven guest RPC features and rejected unsupported sandbox hooks and kernel-module requests. It established a pattern used throughout the reduction: retain operations required for the supported workload contract, and return errors for removed capabilities instead of accepting them as ineffective settings.

The RPC inventory distinguished required lifecycle, process I/O, networking and storage operations from unsupported methods. The recorded validation combined successful ordinary workloads with direct negative RPC probes. Later passes refined the wire defaults and request-field validation; this was the initial RPC reduction, not the final supported-method list.

## About this record

This is a sanitized retrospective summary of the original pass documentation.
The timestamp is its original Git author timestamp, converted to UTC; it is not
the creation time of this public summary or an exact test/deployment time.
Historical validation claims have not been rerun for this documentation update.
For individual behavioral corrections, see the [additive-fix summary](../additive-fixes/README.md).

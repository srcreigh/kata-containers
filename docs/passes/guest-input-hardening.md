# Bound guest inputs and separate telemetry provenance

[Documentation history](../../README.md#documentation-history)

Original documentation record: **2026-09-19 01:37:10 UTC**.
Public implementation reference(s): `3d88e08bd–a0e17099d`.

The hardening pass added cancellation cleanup and pending-request limits to the pinned ttrpc client, bounded frame progress and method payloads, validated stream lengths, and tightened the hybrid-vsock acknowledgement. It constrained OOM identities/rates, pinned termination-file writes, and bounded logs, traces and VMM diagnostics. Host and guest metrics were separated.

The new VMM API deadlines exposed I/O-only Tokio runtimes without timer drivers. Enabling timers repaired that integration regression. This is distinct from the later synchronous-thread-join stall.

Recorded native checks covered selected malformed-input and lifecycle cases; live checks established ordinary compatibility. Collection checks after decoding do not prevent every allocation, and accepted guest values can still be false.

## About this record

This is a sanitized retrospective summary of the original pass documentation.
The timestamp is its original Git author timestamp, converted to UTC; it is not
the creation time of this public summary or an exact test/deployment time.
Historical validation claims have not been rerun for this documentation update.
For individual behavioral corrections, see the [additive-fix summary](../additive-fixes/README.md).

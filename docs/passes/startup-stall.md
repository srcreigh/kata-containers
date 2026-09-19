# Startup-stall diagnosis and validation limits

[Documentation history](../../README.md#documentation-history)

Original documentation record: **2026-09-19 13:42:38 UTC**.
Public implementation reference(s): `6e2a5b2c2`.

This follow-up explained why a previously working startup could stall after reduction. Removing a duplicate vsock API operation changed scheduling before network setup, exposing an existing synchronous join of a dedicated namespace thread from an async worker.

The fix awaited a blocking-task bridge while retaining the dedicated namespace thread. A one-worker regression demonstrated the progress failure and passed with the bridge. The real runtime had multiple workers, and the exact pending production future was not captured: the removed duplicate API call is a plausible trigger, not a proven sole cause.

Timers were already enabled by the earlier repair, so a simple timed HTTP wait does not fully explain the indefinite observation. Cancellation of the caller also does not forcibly cancel the blocking task. These distinctions and the qualified workload/cleanup evidence are preserved in the detailed diagnosis accessible from the behavioral-fix inventory.

## About this record

This is a sanitized retrospective summary of the original pass documentation.
The timestamp is its original Git author timestamp, converted to UTC; it is not
the creation time of this public summary or an exact test/deployment time.
Historical validation claims have not been rerun for this documentation update.
For individual behavioral corrections, see the [additive-fix summary](../additive-fixes/README.md).

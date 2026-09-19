# Unused network replies and API-body reduction

[Documentation history](../../README.md#documentation-history)

Original documentation record: **2026-09-19 00:13:50 UTC**.
Public implementation reference(s): `6f7b87918`.

The host no longer converted UpdateInterface/UpdateRoutes response bodies that had no consumer, and the guest stopped generating those unused replies. Requests and envelope success/error handling remained.

This pass also discarded detailed Firecracker API error bodies to reduce parsing. That decision was later reversed because the diagnostics were useful; subsequent hardening retained error text under explicit size and time bounds. The original record reported successful networking/workload checks. This page records the intermediate decision rather than presenting it as the final implementation.

## About this record

This is a sanitized retrospective summary of the original pass documentation.
The timestamp is its original Git author timestamp, converted to UTC; it is not
the creation time of this public summary or an exact test/deployment time.
Historical validation claims have not been rerun for this documentation update.
For individual behavioral corrections, see the [additive-fix summary](../additive-fixes/README.md).

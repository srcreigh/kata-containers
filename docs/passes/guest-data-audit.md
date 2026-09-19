# Guest-to-host data-flow security audit

[Documentation history](../../README.md#documentation-history)

Original documentation record: **2026-09-19 01:05:00 UTC**.
Public implementation reference(s): `bb812bab2`.

The audit looked beyond method-specific protobuf conversion. It followed RPC framing, stdout/stderr, stdin acknowledgements, OOM IDs, termination messages, guest logs, metrics and tracing, as well as VMM API responses and stderr.

It also considered what happens after data leaves the runtime: containerd events, host files, logging systems, metric collectors and trace exporters retain trust and resource boundaries of their own. An unused parser can be deleted; a necessary consumer instead needs bounded input and clear provenance.

This was analysis, not a claim that remediation had already shipped. A follow-up rechecked the audit against the current source before the hardening pass below.

## About this record

This is a sanitized retrospective summary of the original pass documentation.
The timestamp is its original Git author timestamp, converted to UTC; it is not
the creation time of this public summary or an exact test/deployment time.
Historical validation claims have not been rerun for this documentation update.
For individual behavioral corrections, see the [additive-fix summary](../additive-fixes/README.md).

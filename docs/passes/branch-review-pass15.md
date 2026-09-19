# Pass 15: exhaustive runtime and guest branch review

[Documentation history](../../README.md#documentation-history)

Original documentation record: **2026-09-19 03:39:04 UTC**.
Public implementation reference(s): `85a851063–6e2a5b2c2`.

This pass reviewed retained production branches across the agent, rustjail, host runtime/resources, shared types/utilities and vendored transport. It removed ineffective settings, unreachable alternatives, duplicate plumbing and unnecessary abstractions while preserving error handling and optional Kubernetes/Firecracker functionality.

The review corrected copied-volume monitor ownership, shared block-device reference lifetime, filesystem-versus-raw classification and trace reconnection behavior. It rejected pre-prepare device attachment and propagated the initial device-filesystem mount error.

Historical measurement reported 49,690 to 41,779 tracked Rust code lines, including tests/vendor code but excluding comments and blanks: 7,911 removed (15.9%). Validation recorded 338 distinct passing tests across two phases, four ignored tests and an excluded inherited-stdio test. It did not rerun every suite on the final commit.

Initial untraced startup checks stalled despite a successful traced check. The candidate was corrected before promotion. Subsequent workload checks passed; one multi-volume cleanup waiter failed and cleanup was checked separately. The later stall explanation distinguishes the demonstrated scheduling hazard from its unproven timing trigger.

## About this record

This is a sanitized retrospective summary of the original pass documentation.
The timestamp is its original Git author timestamp, converted to UTC; it is not
the creation time of this public summary or an exact test/deployment time.
Historical validation claims have not been rerun for this documentation update.
For individual behavioral corrections, see the [additive-fix summary](../additive-fixes/README.md).

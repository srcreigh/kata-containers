# Reject unsupported parent-to-child setup messages

[Fix inventory](README.md) · **New internal contract enforcement during simplification**

Implementation history: `85a851063ac8`.

## Before

The async parent writer accepted message kinds that the retained parent-to-child setup protocol never sends, including the child-to-parent failure form. Its generic branches obscured the actual direction of the protocol.

## Change

`write_async` accepts only SYNC_DATA and SYNC_SUCCESS and errors before writing for other message types. Child failures still use the synchronous child writer and are consumed by the parent's async reader. The write helper derives its target length from the slice; redundant successful-length checks were removed.

## Validation and tradeoffs

Rustjail regressions send parent data/acknowledgements, reject a parent SYNC_FAILED, and verify a child error spanning more than one read chunk still reaches the parent. The recorded native rustjail suite passed.

This is a small internal invariant, not a new host/guest transport security fix. Short-write loops and child error propagation were already present. No production failure is attributed to an unsupported parent message in the historical record.

Validation summaries describe the original development checks, not a fresh test run during publication. Deployment logs and operator scripts are outside this source repository. Source-level regressions, where present, remain alongside the implementation.

## Source

- [src/agent/rustjail/src/sync_with_async.rs](../../src/agent/rustjail/src/sync_with_async.rs)
- [src/agent/rustjail/src/sync.rs](../../src/agent/rustjail/src/sync.rs)

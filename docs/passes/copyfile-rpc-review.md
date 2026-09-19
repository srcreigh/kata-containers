# CopyFile and full guest RPC-handler review

[Documentation history](../../README.md#documentation-history)

Original documentation record: **2026-09-18 21:47:17 UTC**.
Public implementation reference(s): `e5684e669–b20a8e6cb`.

CopyFile stopped implicitly creating and changing ownership of parent directories. Sandbox setup creates the shared-copy root; the host sends explicit directory creation in the copying order. Missing parents now fail, and parent metadata is preserved.

The subsequent RPC review removed additional unused raw-device and guest paths, with rejection checks guarding the narrowed contract. The documentation first recorded native validation while live rollout was still pending, then recorded workload verification separately. Raw `volumeDevices` handling was later restored; this historical summary does not describe its removal as the current contract.

## About this record

This is a sanitized retrospective summary of the original pass documentation.
The timestamp is its original Git author timestamp, converted to UTC; it is not
the creation time of this public summary or an exact test/deployment time.
Historical validation claims have not been rerun for this documentation update.
For individual behavioral corrections, see the [additive-fix summary](../additive-fixes/README.md).

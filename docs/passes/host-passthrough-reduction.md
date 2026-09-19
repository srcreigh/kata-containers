# Host passthrough and vsock reduction

[Documentation history](../../README.md#documentation-history)

Original documentation record: **2026-09-18 18:22:34 UTC**.
Public implementation reference(s): `9927303de`.

The host runtime lost VFIO, PCI topology and alternate vsock implementations that no longer had a supported Firecracker consumer. Retained host device construction and guest communication were checked together so an apparently unreachable backend could not still be selected through defaults or configuration.

The recorded rollout exercised ordinary storage, network and workload behavior. This pass did not remove the Firecracker hybrid-vsock connection itself or weaken the jailer, credentials or namespace boundary.

## About this record

This is a sanitized retrospective summary of the original pass documentation.
The timestamp is its original Git author timestamp, converted to UTC; it is not
the creation time of this public summary or an exact test/deployment time.
Historical validation claims have not been rerun for this documentation update.
For individual behavioral corrections, see the [additive-fix summary](../additive-fixes/README.md).

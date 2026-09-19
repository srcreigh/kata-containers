# Host filesystem transport and networking reduction

[Documentation history](../../README.md#documentation-history)

Original documentation record: **2026-09-18 18:46:28 UTC**.
Public implementation reference(s): `51fa7617c–d3008471d`.

Shared host-filesystem transports and vhost-user networking were removed. Projected files continued through explicit copying, and guest-side filesystem mounts remained distinct from a host filesystem-sharing transport.

A candidate exposed unsuitable generic disk-driver defaults after alternate transports were deleted. The follow-up set both boot and container block-driver defaults to MMIO. Recorded workload validation followed that correction. Supported CNI networking remained on the TAP/tcfilter path.

## About this record

This is a sanitized retrospective summary of the original pass documentation.
The timestamp is its original Git author timestamp, converted to UTC; it is not
the creation time of this public summary or an exact test/deployment time.
Historical validation claims have not been rerun for this documentation update.
For individual behavioral corrections, see the [additive-fix summary](../additive-fixes/README.md).

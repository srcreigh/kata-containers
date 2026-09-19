# Accept OOM notifications only for registered containers

[Fix inventory](README.md) · **New host identity and rate validation**

Implementation history: `3d88e08bdc91`.

## Before

The guest supplies the container ID in an OOM event. Forwarding arbitrary IDs lets a guest misattribute events beyond the host's known container set; repeated immediate events can also monopolize work or block shutdown.

## Change

The host maintains a lifecycle-owned registry, rejects empty/overlong/unregistered IDs and suppresses repeated accepted events for one ID within a second. Registration is removed on create failure and deletion; duplicate container creation is rejected. Polling is paced at 100 ms with skipped missed ticks, publication has a one-second bound, and cancellation is checked during waits.

## Validation and tradeoffs

Native registry tests cover membership and deduplication. Historical live resource checks validate ordinary behavior but also record an unresolved end-to-end OOM-status limitation.

Membership does not prove a genuine kernel OOM: the guest can still lie about its own registered containers. Prior tests observed exit 137/Error and differing OOM counters; this patch must not be described as having fixed Kubernetes OOMKilled classification.

Validation summaries describe the original development checks, not a fresh test run during publication. Deployment logs and operator scripts are outside this source repository. Source-level regressions, where present, remain alongside the implementation.

## Source

- [src/runtime-rs/crates/runtimes/virt_container/src/oom.rs](../../src/runtime-rs/crates/runtimes/virt_container/src/oom.rs)
- [src/runtime-rs/crates/runtimes/virt_container/src/sandbox.rs](../../src/runtime-rs/crates/runtimes/virt_container/src/sandbox.rs)
- [src/runtime-rs/crates/runtimes/virt_container/src/container_manager/manager.rs](../../src/runtime-rs/crates/runtimes/virt_container/src/container_manager/manager.rs)

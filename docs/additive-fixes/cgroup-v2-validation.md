# Reject resource controls that the cgroup-v2 guest cannot apply

[Fix inventory](README.md) · **New fork contract enforcement**

Implementation history: `749e1521e801`.

## Before

Deleting cgroup-v1 code while retaining its OCI resource fields would allow updates that appear successful but have no effect. Examples include v1 network class/priorities, CPU realtime controls, kernel-memory controls, swappiness, disable-OOM-killer and block-I/O leaf weight.

## Change

A shared guest validator rejects those unsupported fields before container/update side effects, and the filesystem cgroup manager applies the same validation. Supported CPU, memory, PID, device and hugepage controls remain implemented using the existing v2 machinery.

## Validation and tradeoffs

Native RPC/rustjail rejection tests and live resource/negative probes are recorded in the guest-reduction and deployment reports.

This is deliberate narrowing, not an assertion that those controls are useless in all Kubernetes deployments. It does not add VM memory hotplug or a guest swap disk; per-container memory accounting and limits are separate from VM sizing.

Validation summaries describe the original development checks, not a fresh test run during publication. Deployment logs and operator scripts are outside this source repository. Source-level regressions, where present, remain alongside the implementation.

## Source

- [src/agent/rustjail/src/cgroups/fs/mod.rs](../../src/agent/rustjail/src/cgroups/fs/mod.rs)
- [src/agent/src/rpc.rs](../../src/agent/src/rpc.rs)

# Require the supported PID-1 and container PID-namespace model

[Fix inventory](README.md) · **New fork contract enforcement**

Implementation history: `749e1521e801`, `85a851063ac8`.

## Before

The general agent contained both service-manager/non-PID-1 startup and guest-PID-namespace sharing alternatives. Removing those paths without an explicit guard could run a reduced agent with assumptions its environment does not satisfy.

## Change

Normal agent service startup now requires PID 1; special initialization/version entry points remain separately handled. Container process setup requires a PID namespace to create or join rather than accepting the removed guest-PID sharing path. Existing namespace creation, joining and child initialization perform the actual isolation.

## Validation and tradeoffs

The guest-reduction and branch-review reports trace the guards and retained namespace paths; native namespace/rustjail tests and booted Kubernetes workloads exercise the supported model.

This is not a new namespace isolation implementation and does not establish support for arbitrary standalone OCI runtimes. The exception paths must be understood when testing the agent binary outside its guest image.

Validation summaries describe the original development checks, not a fresh test run during publication. Deployment logs and operator scripts are outside this source repository. Source-level regressions, where present, remain alongside the implementation.

## Source

- [src/agent/src/main.rs](../../src/agent/src/main.rs)
- [src/agent/rustjail/src/container.rs](../../src/agent/rustjail/src/container.rs)
- [src/agent/rustjail/src/validator.rs](../../src/agent/rustjail/src/validator.rs)

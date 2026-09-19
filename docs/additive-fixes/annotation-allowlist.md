# Keep workload annotations inside the supported contract

[Fix inventory](README.md) · **New fork contract enforcement**

Implementation history: `3ae371a3ec84`, `85a851063ac8`.

## Before

Allowing a broad annotation pattern could let a pod request deleted runtime options despite a restrictive base configuration. Simply removing setters risks accepting and ignoring a request.

## Change

The shared annotation validator allows only the retained Kata configuration overrides: default vCPU count, default memory, agent tracing and runtime tracing. Other Kata configuration keys fail even when `enable_annotations` is broad. Existing CPU/memory bounds and value parsing still apply. Ordinary Kubernetes metadata outside the Kata configuration namespace is not an instruction to reconfigure the runtime.

## Validation and tradeoffs

Shared-types and runtime contract tests exercise allowed and rejected annotations; the branch-review ledgers trace validation through task creation.

This narrows upstream's extensibility deliberately. It is not a replacement for Kubernetes admission control. Tracing annotations enable tracing only within the host-controlled configuration; they do not grant a pod arbitrary exporter configuration.

Validation summaries describe the original development checks, not a fresh test run during publication. Deployment logs and operator scripts are outside this source repository. Source-level regressions, where present, remain alongside the implementation.

## Source

- [src/libs/kata-types/src/annotations/mod.rs](../../src/libs/kata-types/src/annotations/mod.rs)
- [src/runtime-rs/crates/runtimes/src/manager.rs](../../src/runtime-rs/crates/runtimes/src/manager.rs)

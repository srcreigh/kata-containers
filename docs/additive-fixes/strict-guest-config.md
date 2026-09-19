# Reject removed guest options, including shadowed command-line options

[Fix inventory](README.md) · **New fork contract enforcement**

Implementation history: `a3385f1e1f38`, `749e1521e801`.

## Before

The guest used to accept a much broader configuration surface. After removing services, silently ignoring their options would hide configuration errors. Validating only the selected configuration file would also miss forbidden `agent.*` kernel arguments present alongside that file.

## Change

The reduced parser rejects unknown TOML fields and unsupported agent kernel options. It validates command-line agent options before choosing a configuration file, preventing that shadowing case. Retained values are checked, and environment overrides are limited to the implemented server address/log level behavior. Cgroup-v1 mode is rejected; tracing remains an explicit supported opt-in following its restoration.

## Validation and tradeoffs

Guest config tests cover the reduced input paths; historical live rejection probes verify the guest starts under the corresponding generated host configuration.

This is a stricter supported-input policy, not a complete validation of arbitrary Linux kernel arguments. Existing configurations containing removed options now fail and must be updated. The related host-default regression has its own page.

Validation summaries describe the original development checks, not a fresh test run during publication. Deployment logs and operator scripts are outside this source repository. Source-level regressions, where present, remain alongside the implementation.

## Source

- [src/agent/src/config.rs](../../src/agent/src/config.rs)

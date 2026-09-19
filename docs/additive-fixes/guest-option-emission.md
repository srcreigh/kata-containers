# Stop the host from generating options its guest rejects

[Fix inventory](README.md) · **Regression introduced and repaired in the fork**

Implementation history: `a3385f1e1f38`, `5808dd87e542`.

## Before

The startup reduction made the guest reject removed options, but the host still emitted them from defaults. In particular, the nonzero CDH timeout generated `agent.cdh_api_timeout` even when the removed confidential-data service was not being used. The canary failed before agent hybrid-vsock became available.

## Change

The follow-up deleted obsolete host agent fields/defaults and their kernel-argument emission. It tested both programmatic and deserialized defaults because their debug behavior differs. At that stage only log level and pipe size were emitted; opt-in `agent.trace` was restored later with tracing support.

## Validation and tradeoffs

The startup/IO deployment record identifies the failed `a3385f1e1` attempt and `5808dd87e` correction. The configuration tests check emitted keys, not just that parsing succeeds.

This was our host/guest contract regression, not a pre-existing upstream boot failure. The general lesson is to test producer defaults against the reduced consumer: removing the service and validating inputs are insufficient if the host still generates those inputs.

Validation summaries describe the original development checks, not a fresh test run during publication. Deployment logs and operator scripts are outside this source repository. Source-level regressions, where present, remain alongside the implementation.

## Source

- [src/libs/kata-types/src/config/mod.rs](../../src/libs/kata-types/src/config/mod.rs)
- [src/libs/kata-types/src/config/agent.rs](../../src/libs/kata-types/src/config/agent.rs)
- [src/agent/src/config.rs](../../src/agent/src/config.rs)

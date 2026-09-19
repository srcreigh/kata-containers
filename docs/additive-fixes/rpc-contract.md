# Return explicit errors for removed RPCs and request fields

[Fix inventory](README.md) · **New fork contract enforcement**

Implementation history: `c31d596f9901`, `a3385f1e1f38`, `065454d4cc97`, `749e1521e801`.

## Before

Removing a handler or ignoring a request field can make an unsupported operation look like a successful no-op, or allow unrelated setup before failure. The retained wire schema still contains methods/fields that this fork does not implement.

## Change

Guest preflight rejects removed sandbox/container features, including hook/module/shared-mount and passfd fields, before relevant locks and mutations. Generated unsupported service defaults use UNIMPLEMENTED. Host management resize/policy endpoints return HTTP 501 before requesting the removed agent behavior. Necessary ordinary request validation remains.

## Validation and tradeoffs

The RPC inventory and native tests distinguish retained handlers from explicit rejection. The final live probe records 49 unsupported-request cases plus the retained empty-routes case.

The protobuf definitions are not all deleted: wire compatibility and explicit failures are useful. This does not make every unknown future protobuf field semantically validated, nor make accepted RPCs transactional. Kubernetes exec, streams and supported resource updates remain functional.

Validation summaries describe the original development checks, not a fresh test run during publication. Deployment logs and operator scripts are outside this source repository. Source-level regressions, where present, remain alongside the implementation.

## Source

- [src/agent/src/rpc.rs](../../src/agent/src/rpc.rs)
- [src/libs/protocols/build.rs](../../src/libs/protocols/build.rs)
- [src/runtime-rs/crates/runtimes/src/shim_mgmt/handlers.rs](../../src/runtime-rs/crates/runtimes/src/shim_mgmt/handlers.rs)

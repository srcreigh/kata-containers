# Make CopyFile preserve parent-directory metadata

[Fix inventory](README.md) · **Correction and deliberate narrowing of inherited behavior**

Implementation history: `e5684e66918b`.

## Before

CopyFile implicitly created missing parents and changed their ownership. Copying one file could therefore mutate shared directory metadata, affecting other containers or files. That behavior was unnecessary because the host already knows the directory tree it is copying.

## Change

CopyFile now requires the parent to exist and applies requested ownership/mode to the target, not the parents. Sandbox setup creates the shared-copy root once. Host recursive copying sends explicit directory creation requests in the required order. Missing parents produce errors for both file and directory targets.

## Validation and tradeoffs

Native RPC regressions check missing parents and unchanged parent metadata. The RPC-review rollout includes negative parent probes and successful ordinary copied-file workloads.

Callers must create directories explicitly; this intentionally changes the RPC contract. Existing path checks remain necessary and are not replaced by this rule. The fix does not imply that every CopyFile failure is transactional or that all copied-volume races are solved.

Validation summaries describe the original development checks, not a fresh test run during publication. Deployment logs and operator scripts are outside this source repository. Source-level regressions, where present, remain alongside the implementation.

## Source

- [src/agent/src/rpc.rs](../../src/agent/src/rpc.rs)
- [src/agent/src/sandbox.rs](../../src/agent/src/sandbox.rs)
- [src/runtime-rs/crates/resource/src/volume/copy_volume.rs](../../src/runtime-rs/crates/resource/src/volume/copy_volume.rs)

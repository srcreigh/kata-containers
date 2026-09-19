# Reject unsupported storage requests before mount side effects

[Fix inventory](README.md) · **New fork contract enforcement**

Implementation history: `c31d596f9901`, `065454d4cc97`, `85a851063ac8`.

## Before

Once alternate storage drivers and shared-filesystem hooks were removed, merely dropping their branches could silently accept unsupported metadata or fail only after earlier mounts had already changed guest state.

## Change

Storage preflight checks the request set before the mounting loop. The retained drivers are MMIO block, ephemeral and local storage, with only their implemented options. The old special shared-filesystem premount shortcut is rejected instead of being accepted without doing useful work. Driver-specific validation continues in the concrete handlers.

## Validation and tradeoffs

Native storage/RPC tests and live unsupported-request probes cover the contract. The pass-15 agent ledger follows validation ordering and the retained block/filesystem paths.

Preflight prevents unsupported-feature side effects; it is not a transaction or rollback mechanism for later I/O failures. Hugepages remain available through ephemeral storage. Retained mount options must not be mistaken for arbitrary driver options.

Validation summaries describe the original development checks, not a fresh test run during publication. Deployment logs and operator scripts are outside this source repository. Source-level regressions, where present, remain alongside the implementation.

## Source

- [src/agent/src/storage/mod.rs](../../src/agent/src/storage/mod.rs)
- [src/agent/src/storage/block_handler.rs](../../src/agent/src/storage/block_handler.rs)
- [src/agent/src/rpc.rs](../../src/agent/src/rpc.rs)

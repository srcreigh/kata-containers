# Pin the host termination file and cap guest termination text

[Fix inventory](README.md) · **Host file-write hardening**

Implementation history: `3d88e08bdc91`.

## Before

Termination text crosses from the guest back into a host file named by the pod's mount configuration. Reopening by pathname at exit permits the target to change; an unlimited diagnostic string also exceeds Kubernetes' useful per-container message size.

## Change

For File policy, the runtime resolves the matching mount and opens its existing absolute host source before execution through a pinned parent, with NOFOLLOW/NONBLOCK/CLOEXEC. It requires a regular file with one link and retains that file descriptor. Exit writes use that same file, capped at 4096 bytes on a UTF-8 boundary. The guest diagnostic reader also bounds its read to 4096 bytes.

## Validation and tradeoffs

The native test covers target replacement/pinning and truncation. A live canary emitted 8192 bytes and observed the bounded message.

The text remains guest-controlled. This does not introduce a new fallback-to-logs implementation, nor authenticate the message. A changed pathname no longer redirects the retained descriptor, but an open file can still be modified by other authorized writers.

Validation summaries describe the original development checks, not a fresh test run during publication. Deployment logs and operator scripts are outside this source repository. Source-level regressions, where present, remain alongside the implementation.

## Source

- [src/runtime-rs/crates/runtimes/virt_container/src/termination.rs](../../src/runtime-rs/crates/runtimes/virt_container/src/termination.rs)
- [src/runtime-rs/crates/runtimes/virt_container/src/container_manager/container.rs](../../src/runtime-rs/crates/runtimes/virt_container/src/container_manager/container.rs)
- [src/agent/src/rpc.rs](../../src/agent/src/rpc.rs)

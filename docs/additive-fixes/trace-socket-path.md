# Bind trace sockets under long sandbox paths without replacing listeners

[Fix inventory](README.md) · **Integration correction during restoration**

Implementation history: `bb812bab2ed0`.

## Before

A Firecracker sandbox's full trace-socket pathname can exceed the Unix socket address length even though the filesystem path itself is valid. Blindly unlinking an existing socket before binding would also interfere with a running forwarder.

## Change

The forwarder opens the parent directory and binds using `/proc/self/fd/<fd>/<basename>`, shortening the address passed to bind without changing process working directory. The resulting socket exists in the intended parent. It does not unlink an existing listener; binding then fails normally.

## Validation and tradeoffs

The native regression uses a long parent path, verifies the socket exists at its intended location, and verifies a second bind fails without replacing it. The restored tracing canary exercises real sandbox paths.

The approach is Linux-specific and requires procfs. It does not remove the socket basename length constraint or implement automatic stale-socket cleanup. Privilege dropping in the forwarder is retained behavior, not a newly introduced fix.

Validation summaries describe the original development checks, not a fresh test run during publication. Deployment logs and operator scripts are outside this source repository. Source-level regressions, where present, remain alongside the implementation.

## Source

- [src/tools/trace-forwarder/src/main.rs](../../src/tools/trace-forwarder/src/main.rs)

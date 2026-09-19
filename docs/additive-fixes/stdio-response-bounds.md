# Validate guest stream lengths and remove borrowed-future lifetime tricks

[Fix inventory](README.md) · **New host input hardening**

Implementation history: `3d88e08bdc91`.

## Before

Guest stdout/stderr lengths and stdin acknowledgements reach host AsyncRead/AsyncWrite adapters. An oversized read can exceed the caller's buffer; an acknowledgement larger than the submitted input violates the write contract. Stored futures also used an unsafe lifetime workaround.

## Change

Read completion checks both the originally requested length and the current ReadBuf capacity before copying. Write completion checks the original submission and current buffer, and an empty shutdown write must acknowledge zero bytes. Stored futures own cloned agent/context and request data, allowing ordinary static futures without lifetime transmutation.

## Validation and tradeoffs

Native container-I/O regressions exercise malicious lengths. Historical live exec, TTY, binary I/O and normal stream tests check compatibility.

The checks prevent these particular host API-contract violations; they do not make guest output truthful or sanitize terminal content. Partial writes and application-level backpressure still apply, and transport limits are separate.

Validation summaries describe the original development checks, not a fresh test run during publication. Deployment logs and operator scripts are outside this source repository. Source-level regressions, where present, remain alongside the implementation.

## Source

- [src/runtime-rs/crates/runtimes/virt_container/src/container_manager/io/container_io.rs](../../src/runtime-rs/crates/runtimes/virt_container/src/container_manager/io/container_io.rs)

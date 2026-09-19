# Reject removed RPC transport schemes before creating sockets

[Fix inventory](README.md) · **New fork transport contract enforcement**

Implementation history: `fe8a41b4e9ff`.

## Before

The vendored transport library retained TCP and platform-specific address forms even though this fork uses Unix sockets on the host and vsock inside the guest. Deleting only connection branches could leave ambiguous parsing or a fallback to an unintended socket family.

## Change

Transport selection explicitly rejects removed and unknown schemes before socket creation. The supported Unix filesystem/abstract and vsock paths remain. TCP listeners/connectors and Windows/platform compatibility implementations are removed rather than hidden behind a selectable build option. Both synchronous and asynchronous APIs remain because the containerd dependency still needs the former.

## Validation and tradeoffs

Native transport rejection tests cover IPv4/IPv6 TCP, named-pipe and unknown schemes. The vendored patch record separates this contract change from the earlier async frame and request-lifetime hardening.

This is an intentional reduction of library capabilities, not a claim that upstream TCP was insecure. Unix and vsock still require their normal peer/access assumptions. It does not make the async frame deadline apply to the retained synchronous implementation.

Validation summaries describe the original development checks, not a fresh test run during publication. Deployment logs and operator scripts are outside this source repository. Source-level regressions, where present, remain alongside the implementation.

## Source

- [vendor/ttrpc/src/asynchronous/transport/mod.rs](../../vendor/ttrpc/src/asynchronous/transport/mod.rs)
- [vendor/ttrpc/src/common.rs](../../vendor/ttrpc/src/common.rs)
- [vendor/ttrpc/KATA-PATCHES.md](../../vendor/ttrpc/KATA-PATCHES.md)

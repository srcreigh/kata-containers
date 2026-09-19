# Make the agent-only guest bootable

[Fix inventory](README.md) · **Fork integration and regression repair**

Implementation history: `13a5da3aae84`, `bbdca6a60cbc`.

## Before

Replacing the general guest image with one static PID-1 agent removed the distribution machinery that normally provides its filesystem layout. A filesystem image alone also does not match the retained kernel command line: the root device is `/dev/vda1`, not the whole `/dev/vda` disk. A real `/var/run` on the read-only root prevents creation of persistent namespace files.

## Change

The image builder creates an MBR disk with an ext4 partition at byte 1,048,576, installs `/sbin/init` as a symlink to the static agent, and makes `/var/run` point to writable `/run`. It supplies minimal passwd/group, hostname and resolv.conf files. It requires root ownership, checks that the agent has no ELF interpreter, and refuses to overwrite an existing image. The 65 MiB disk contains a 64 MiB filesystem.

## Validation and tradeoffs

The initial deployment record explicitly records the `/var/run` startup failure and correction; subsequent canaries and workload validation booted this image layout.

This adapts our custom image; it is not evidence that upstream distribution images were broken. The image intentionally omits a shell and filesystem-formatting utilities. Changing root partitioning requires keeping kernel parameters and image construction in agreement.

Validation summaries describe the original development checks, not a fresh test run during publication. Deployment logs and operator scripts are outside this source repository. Source-level regressions, where present, remain alongside the implementation.

## Source

- [src/runtime-rs/crates/hypervisor/src/kernel_param.rs](../../src/runtime-rs/crates/hypervisor/src/kernel_param.rs)

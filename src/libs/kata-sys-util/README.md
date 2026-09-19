# `kata-sys-util`

Linux utilities shared by the Firecracker runtime and guest agent.

- Filesystem basename extraction for hugepage volumes.
- Bind mounts, remounts, propagation, mountpoint creation, mount-option validation,
  and `/proc/mounts` inspection. Callers must validate mount destination paths.
- Kubernetes emptyDir and hugepage volume identification.
- Network namespace switching with an RAII guard and namespace name generation.
- OCI bundle loading and container/sandbox annotation interpretation.
- Container/exec ID and environment-variable validation.
- Bounded guest log and control-line reads.
- Random bytes and UUID generation.

The supported platform is x86_64 Linux, as specified in the repository README.

This code is licensed under [Apache-2.0](../../../LICENSE).

# kata-types

Shared types and configuration for the reduced Kubernetes/Firecracker runtime.

The crate retains CRI-containerd, CRI-O and dockershim annotations, Firecracker and
agent TOML configuration with ordered drop-ins, Kubernetes volume classification,
CPU sizing, device validation, and direct-volume CSI metadata. Workload configuration
annotations can change only VM CPU/memory sizing and host/agent tracing; unsupported
configuration annotations fail explicitly.

Unsupported configuration fields remain represented where required to reject them.
There are no alternate hypervisor implementations, rootless runtime paths,
hypervisor capability dispatch, guest-pull/Nydus virtual-volume parsers, or separate
global active/default configuration cache.

`safe-path` enables the Linux direct-volume path helpers. `enable-vendor` retains
configuration customization hooks; the deployed runtime uses the default configuration.

License: Apache-2.0; see [LICENSE](../../../LICENSE).

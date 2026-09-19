# Firecracker runtime

This directory contains the Rust containerd shim for the reduced `kata-fc` fork.
It runs Kubernetes containers inside Firecracker VMs using the Kata guest agent.
The [root README](../../README.md) defines the supported workload contract,
build prerequisites, deployment rules and rollback references.

## Implementation

| Crate | Responsibility |
| --- | --- |
| `shim` | Containerd shim v2 `start`, `run` and `delete` commands; journald logging |
| `service` | Containerd task and sandbox ttRPC services and event forwarding |
| `runtimes` | The single `VirtContainer` implementation, configuration, tracing and management endpoints |
| `resource` | VM sizing, sandbox cgroups, tcfilter networking, block roots and volumes |
| `hypervisor` | Firecracker API, jailer lifecycle, MMIO block and network devices |
| `agent` | Kata agent RPCs and log forwarding over Firecracker hybrid vsock |
| `persist` | JSON VM/cgroup state written inside the Firecracker jail root |

There are no alternate VMM or Linux/Wasm runtime implementations or `virt` build
feature. Firecracker startup configures hybrid vsock directly before boot; block
and network devices use the device manager. The host requires the jailer and VMM
seccomp, rejects rootless operation and uses static VM CPU/RAM allocation.
Whole-sandbox host cgroups support systemd or cgroupfs; the guest uses cgroup v2.

Containerd APIs retain container lifecycle, exec/attach/TTY, signals, guest resource
updates and stats. Supported storage includes devmapper container roots,
filesystem CSI volumes, writable raw block devices, hugepages, guest-local volumes
and copied projected files. See the [RPC inventory](../../docs/kata-agent-rpcs.md)
and [unsupported functionality inventory](../../docs/unsupported-functionality.md)
for precise limits and rejection behavior.

The management socket exposes host metrics at `/metrics` and separate guest
metrics at `/metrics/guest`. Host and agent distributed tracing remain optional;
see [tracing setup](../../docs/tracing-fc.md).

## Configuration

The Firecracker template is [configuration-rs-fc.toml.in](config/configuration-rs-fc.toml.in).
Configuration loads from the configured file and its ordered `config.d` drop-ins.
Workload configuration annotations allow only default VM CPU/memory sizing and
host/agent tracing. Unsupported features fail explicitly; compatibility fields in
shared configuration types do not imply an implementation exists.

VM state is saved at `/run/kata/firecracker/<sandbox-id>/root/state.json`.
Deployment bundles and active containerd configuration are documented in
[deployment evidence](../../ci/fc/DEPLOYMENT.md).

## Build and validation

From the repository root on x86-64 Linux, use the pinned toolchain and prerequisites
listed in the root README:

```sh
ci/fc/build.sh
```

This generates version/config sources and builds the static shim, agent and trace
forwarder using the committed lockfile. Artifacts and checksums are placed in `dist/`.
Guest image and Firecracker packaging instructions are in the root README.

Native tests are selected by the changed crates; mount/network/process tests need
disposable namespaces and the documented exclusions. They should not be run
unisolated on production hosts. The [pass 15 report](../../docs/branch-review-pass15.md)
records the production branch review, test scope and validation results.

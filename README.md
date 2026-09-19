# kata-fc

An experimental Firecracker-only reduction of Kata Containers for Kubernetes,
based on upstream 4.2.0 (`c7351e797efff8bfc6bd73da0eb1909be12e2cfe`).
Apache-2.0; upstream licenses and attribution remain intact.

The [behavioral-fix inventory](docs/additive-fixes/README.md) explains corrections,
hardening, restorations and regressions introduced during reduction. Reducing code
size is not a security certification; upstream security maintenance remains necessary.

## Supported contract

- x86-64 Linux, jailed Firecracker with VMM seccomp enabled, containerd shim v2.
- Agent-only guest userspace: PID 1, cgroup v2, no systemd/D-Bus or OCI hooks.
- Kubernetes CRI pods with devmapper block-backed roots and MMIO block storage.
- Static VM CPU/RAM sizing, tcfilter networking and hybrid-vsock agent I/O.
- CSI filesystem volumes and read-write raw `volumeDevices`; read-only raw
  attachments fail because the retained drive pool cannot honor that setting.
  Read-only filesystem mounts remain supported.
- Guest-local/ephemeral volumes, hugepages, projected file updates, lifecycle,
  exec/attach/TTY, logging, signals, accounting and normal guest isolation.
- Guest-side NFS with an appropriately configured kernel; no host filesystem-sharing
  transport is introduced.
- Default CPU/memory annotations and opt-in runtime/agent tracing; see
  [tracing setup](docs/tracing-fc.md). Profiling is not implemented.

Unsupported requests fail explicitly. There is no fallback to a host container,
another VMM, alternate disk transport or removed backend. Jailer, seccomp, safe
paths, namespace isolation and credential checks remain essential.

## Native build

Use the pinned Rust toolchain with make, protoc, pkg-config and a Linux x86-64 C
compiler/static libc. Generate the configuration/version source before Cargo:

```sh
make -B -C src/runtime-rs crates/shim/src/config.rs PREFIX=/opt/kata HYPERVISOR=firecracker USE_BUILTIN_DB=false USE_OPENVMM=false
make -B -C src/agent src/version.rs
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS='-C target-feature=+crt-static'
cargo build --locked --release --target x86_64-unknown-linux-gnu -p runtime-rs --bin containerd-shim-kata-v2 -p kata-agent --bin kata-agent -p kata-trace-forwarder --bin kata-trace-forwarder
```

The guest image must supply the PID-1 agent and compatible root partition/layout;
the binaries alone are not a deployable Kubernetes installation. Packaging and
cluster-specific CI/deployment scripts are maintained outside this source repository.
Rust regression tests remain in-tree. Privileged mount, namespace and cgroup tests
need an isolated Linux test environment; do not run them indiscriminately on a host.

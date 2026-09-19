# kata-fc

An experimental Firecracker-only reduction of Kata Containers for Kubernetes,
based on upstream 4.2.0 (`c7351e797efff8bfc6bd73da0eb1909be12e2cfe`).
Apache-2.0; upstream licenses and attribution remain intact.

The [behavioral-fix inventory](docs/additive-fixes/README.md) explains corrections,
hardening, restorations and regressions introduced during reduction. Reducing code
size is not a security certification; upstream security maintenance remains necessary.

## Documentation history

Pass-level reading order, oldest first. Times are the original documentation
commits' author timestamps in **UTC**, not test/deployment times. The linked pass
summaries were reconstructed for this public edition. Individual fix pages are
indexed only through the additive-fix summary at the end.

| Recorded (UTC) | Pass / document |
|---|---|
| 2026-09-18 14:49:17 | [Initial Firecracker-only reduction](docs/passes/initial-reduction.md) |
| 2026-09-18 15:29:20 | [Guest RPC surface reduction](docs/passes/guest-rpc-reduction.md) |
| 2026-09-18 16:14:21 | [Guest device and service reduction](docs/passes/guest-device-reduction.md) |
| 2026-09-18 17:00:21 | [Guest startup and I/O reduction](docs/passes/startup-io-reduction.md) |
| 2026-09-18 18:22:34 | [Host passthrough and vsock reduction](docs/passes/host-passthrough-reduction.md) |
| 2026-09-18 18:46:28 | [Host filesystem transport and networking reduction](docs/passes/filesystem-network-reduction.md) |
| 2026-09-18 19:05:01 | [MMIO-only disk transport and swap-worker reduction](docs/passes/block-transport-reduction.md) |
| 2026-09-18 21:47:17 | [CopyFile and full guest RPC-handler review](docs/passes/copyfile-rpc-review.md) |
| 2026-09-18 22:48:55 | [Host inventory of agent-produced data and parsing reduction](docs/passes/host-input-review.md) |
| 2026-09-19 00:13:50 | [Unused network replies and API-body reduction](docs/passes/network-reply-reduction.md) |
| 2026-09-19 01:05:00 | [Guest-to-host data-flow security audit](docs/passes/guest-data-audit.md) |
| 2026-09-19 01:08:59 | [Restore useful Kubernetes/Firecracker features](docs/passes/feature-restoration.md) |
| 2026-09-19 01:37:10 | [Bound guest inputs and separate telemetry provenance](docs/passes/guest-input-hardening.md) |
| 2026-09-19 01:51:23 | [Pass 12: PID-1 guest and cgroup-v2 reduction](docs/passes/guest-reduction-pass12.md) |
| 2026-09-19 02:23:31 | [Pass 13: unused runtime and guest machinery](docs/passes/guest-reduction-pass13.md) |
| 2026-09-19 03:39:04 | [Pass 15: exhaustive runtime and guest branch review](docs/passes/branch-review-pass15.md) |
| 2026-09-19 13:42:38 | [Startup-stall diagnosis and validation limits](docs/passes/startup-stall.md) |
| 2026-09-19 14:14:55 | [Additive-fix inventory and detailed explanations](docs/additive-fixes/README.md) |

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

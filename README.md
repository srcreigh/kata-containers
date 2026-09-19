# kata-fc

An experimental Firecracker-only reduction of Kata Containers for Kubernetes,
based on upstream 4.2.0 (`c7351e797efff8bfc6bd73da0eb1909be12e2cfe`).
Apache-2.0; upstream licenses and attribution remain intact.

The [behavioral-fix inventory](docs/additive-fixes/README.md) explains corrections,
hardening, restorations and regressions introduced during reduction. Reducing code
size is not a security certification; upstream security maintenance remains necessary.

## Documentation by development pass

Oldest to newest, ordered by the first implementation pass covered by each page.
The explanations were written retrospectively; pages also describe later follow-up
changes. This lists the public technical documents retained from those passes.

1. **Initial Firecracker contract:**
    [Keep workload annotations inside the supported contract](docs/additive-fixes/annotation-allowlist.md);
    [Reject unsupported runtime settings before VM work](docs/additive-fixes/host-contract.md).

2. **Minimal PID-1 guest:**
    [Make the agent-only guest bootable](docs/additive-fixes/guest-image-layout.md).

3. **Hybrid-vsock selection:**
    [Select Firecracker hybrid-vsock consistently](docs/additive-fixes/hybrid-vsock-selection.md).

4. **CSI registration compatibility:**
    [Accept the CSI block-registration format actually supplied by the host](docs/additive-fixes/csi-blk-format.md).

5. **CSI filesystem ownership:**
    [Carry CSI ownership intent into the guest](docs/additive-fixes/csi-fsgroup.md).

6. **Guest RPC and storage restrictions:**
    [Return explicit errors for removed RPCs and request fields](docs/additive-fixes/rpc-contract.md);
    [Reject unsupported storage requests before mount side effects](docs/additive-fixes/storage-preflight.md).

7. **OCI device and hook preflight:**
    [Reject removed OCI device and hook features before setup](docs/additive-fixes/oci-preflight.md).

8. **Physical network rejection:**
    [Reject physical network passthrough before host rebinding](docs/additive-fixes/physical-network-rejection.md).

9. **Guest startup configuration:**
    [Stop the host from generating options its guest rejects](docs/additive-fixes/guest-option-emission.md);
    [Reject removed guest options, including shadowed command-line options](docs/additive-fixes/strict-guest-config.md).

10. **MMIO defaults:**
    [Default both Firecracker disk drivers to MMIO](docs/additive-fixes/mmio-defaults.md).

11. **CopyFile parent handling:**
    [Make CopyFile preserve parent-directory metadata](docs/additive-fixes/copyfile-parent-contract.md).

12. **Host agent-input reduction:**
    [Stop decoding RPC bodies whose only consumer needs success or failure](docs/additive-fixes/ack-only-rpc.md);
    [Bound guest log records and preserve their provenance](docs/additive-fixes/guest-log-boundary.md);
    [Require containerd event publishing at startup](docs/additive-fixes/required-event-publisher.md).

13. **Firecracker metrics panic:**
    [Remove the reachable unimplemented Firecracker metrics call](docs/additive-fixes/vmm-metrics-panic.md).

14. **Raw block, tracing and API diagnostics restoration:**
    [Restore useful API errors with bounded bodies and deadlines](docs/additive-fixes/firecracker-api-errors.md);
    [Restore Kubernetes raw block devices with explicit MMIO mappings](docs/additive-fixes/raw-block-mapping.md);
    [Restore linked tracing and a compatible host exporter](docs/additive-fixes/trace-integration.md);
    [Tracing setup](docs/tracing-fc.md).

15. **Restoration follow-up fixes:**
    [Reject raw disks whose read-only intent cannot be honored](docs/additive-fixes/readonly-raw-block.md);
    [Bind trace sockets under long sandbox paths without replacing listeners](docs/additive-fixes/trace-socket-path.md).

16. **Guest-input bounds and telemetry provenance:**
    [Read an exact, bounded Firecracker connection acknowledgement](docs/additive-fixes/hvsock-handshake.md);
    [Separate host metrics from guest metrics](docs/additive-fixes/metrics-origin.md);
    [Enable timers in the runtimes used for network namespace work](docs/additive-fixes/network-runtime-timers.md);
    [Accept OOM notifications only for registered containers](docs/additive-fixes/oom-event-boundary.md);
    [Limit consumed RPC payloads and selected collections](docs/additive-fixes/response-payload-limits.md);
    [Validate guest stream lengths and remove borrowed-future lifetime tricks](docs/additive-fixes/stdio-response-bounds.md);
    [Pin the host termination file and cap guest termination text](docs/additive-fixes/termination-file.md);
    [Bound guest trace frames and assign host-controlled exporter identity](docs/additive-fixes/trace-input-boundary.md);
    [Bound frame progress and dispatch work from the peer](docs/additive-fixes/ttrpc-frame-progress.md);
    [Bound outstanding RPC work and clean up cancelled unary calls](docs/additive-fixes/ttrpc-request-lifetime.md);
    [Observe VMM exit independently of stderr completion](docs/additive-fixes/vmm-exit-observation.md);
    [Stop unbounded VMM logging without closing its pipe](docs/additive-fixes/vmm-stderr-bounds.md).

17. **PID-1 and cgroup-v2 reduction:**
    [Reject resource controls that the cgroup-v2 guest cannot apply](docs/additive-fixes/cgroup-v2-validation.md);
    [Require the supported PID-1 and container PID-namespace model](docs/additive-fixes/pid1-contract.md);
    [Apply a converted swap limit even when its value is zero](docs/additive-fixes/zero-swap-limit.md).

18. **Unused machinery and saved-state restrictions:**
    [Apply jailer and cgroup restrictions to restored state](docs/additive-fixes/saved-state-contract.md);
    [Reject removed RPC transport schemes before creating sockets](docs/additive-fixes/transport-schemes.md);
    [Do not construct jailed cleanup paths from an empty VM path](docs/additive-fixes/unprepared-vm-cleanup.md).

19. **Runtime and guest branch review:**
    [Keep shared block devices registered until their final user leaves](docs/additive-fixes/block-reference-lifetime.md);
    [Give each copied volume its own update monitor](docs/additive-fixes/copy-monitor-lifetime.md);
    [Fail device attachment before VMM preparation](docs/additive-fixes/device-preparation-state.md);
    [Propagate failure of the initial device-filesystem mount](docs/additive-fixes/devtmpfs-error.md);
    [Reject unsupported parent-to-child setup messages](docs/additive-fixes/parent-child-sync-contract.md);
    [Discard every failed guest trace connection before the next batch](docs/additive-fixes/trace-reconnect.md);
    [Choose filesystem versus raw block from the declared type](docs/additive-fixes/volume-filesystem-selection.md).

20. **Startup-stall repair:**
    [Network namespace thread join and startup stall](docs/additive-fixes/network-thread-stall.md).

21. **Collected explanations:** [Behavioral-fix inventory, grouped by topic](docs/additive-fixes/README.md).

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

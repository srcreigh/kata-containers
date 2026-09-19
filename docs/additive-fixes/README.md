# Behavioral fixes in the Firecracker reduction

This inventory distinguishes corrections to inherited behavior, new validation
for the reduced contract, restorations and regressions introduced by the reduction.
It does not describe all of them as upstream bugs. The baseline is upstream
`c7351e797efff8bfc6bd73da0eb1909be12e2cfe` (4.2.0), not a moving upstream main.
Commit identifiers refer to this repository's publication history.

Operator-specific deployment tools and records are maintained separately.
Historical validation summaries are not independently reproduced by publishing
these pages. The retained in-tree tests provide source-level regression evidence;
live workload checks and their limitations are identified separately.

## Boot and configuration

- [Make the agent-only guest bootable](guest-image-layout.md) — Fork integration and regression repair.
- [Select Firecracker hybrid-vsock consistently](hybrid-vsock-selection.md) — Fork regression repair exposing an upstream capability mismatch.
- [Default both Firecracker disk drivers to MMIO](mmio-defaults.md) — Fork regression repair and explicit configuration validation.
- [Reject unsupported runtime settings before VM work](host-contract.md) — New fork contract enforcement.
- [Keep workload annotations inside the supported contract](annotation-allowlist.md) — New fork contract enforcement.
- [Reject removed guest options, including shadowed command-line options](strict-guest-config.md) — New fork contract enforcement.
- [Stop the host from generating options its guest rejects](guest-option-emission.md) — Regression introduced and repaired in the fork.
- [Require the supported PID-1 and container PID-namespace model](pid1-contract.md) — New fork contract enforcement.
- [Apply jailer and cgroup restrictions to restored state](saved-state-contract.md) — New fork contract enforcement.
- [Propagate failure of the initial device-filesystem mount](devtmpfs-error.md) — Correction to inherited error handling.

## Storage and resources

- [Accept the CSI block-registration format actually supplied by the host](csi-blk-format.md) — Integration compatibility correction.
- [Carry CSI ownership intent into the guest](csi-fsgroup.md) — Integration correction and new metadata validation.
- [Make CopyFile preserve parent-directory metadata](copyfile-parent-contract.md) — Correction and deliberate narrowing of inherited behavior.
- [Give each copied volume its own update monitor](copy-monitor-lifetime.md) — Correction to inherited monitor ownership.
- [Keep shared block devices registered until their final user leaves](block-reference-lifetime.md) — Correction to inherited reference bookkeeping.
- [Choose filesystem versus raw block from the declared type](volume-filesystem-selection.md) — Correction to inherited destination-name heuristic.
- [Restore Kubernetes raw block devices with explicit MMIO mappings](raw-block-mapping.md) — Restoration plus fork-specific validation.
- [Reject raw disks whose read-only intent cannot be honored](readonly-raw-block.md) — Safety correction during feature restoration.
- [Reject unsupported storage requests before mount side effects](storage-preflight.md) — New fork contract enforcement.
- [Reject resource controls that the cgroup-v2 guest cannot apply](cgroup-v2-validation.md) — New fork contract enforcement.
- [Apply a converted swap limit even when its value is zero](zero-swap-limit.md) — Correction to inherited cgroup-v2 limit handling.
- [Fail device attachment before VMM preparation](device-preparation-state.md) — Correction to inherited ineffective queuing.

## RPC and input contracts

- [Reject removed OCI device and hook features before setup](oci-preflight.md) — New fork contract enforcement.
- [Reject physical network passthrough before host rebinding](physical-network-rejection.md) — New fork safety boundary.
- [Return explicit errors for removed RPCs and request fields](rpc-contract.md) — New fork contract enforcement.
- [Reject unsupported parent-to-child setup messages](parent-child-sync-contract.md) — New internal contract enforcement during simplification.
- [Require containerd event publishing at startup](required-event-publisher.md) — New fork integration invariant.
- [Stop decoding RPC bodies whose only consumer needs success or failure](ack-only-rpc.md) — Input-surface correction primarily achieved by deletion.
- [Reject removed RPC transport schemes before creating sockets](transport-schemes.md) — New fork transport contract enforcement.

## Guest data and transport

- [Validate guest stream lengths and remove borrowed-future lifetime tricks](stdio-response-bounds.md) — New host input hardening.
- [Bound outstanding RPC work and clean up cancelled unary calls](ttrpc-request-lifetime.md) — Patch to pinned upstream ttrpc 0.9.0.
- [Bound frame progress and dispatch work from the peer](ttrpc-frame-progress.md) — Patch to pinned upstream ttrpc 0.9.0.
- [Limit consumed RPC payloads and selected collections](response-payload-limits.md) — New host input hardening.
- [Read an exact, bounded Firecracker connection acknowledgement](hvsock-handshake.md) — Host transport hardening.
- [Bound guest log records and preserve their provenance](guest-log-boundary.md) — Host input hardening and parser removal.
- [Accept OOM notifications only for registered containers](oom-event-boundary.md) — New host identity and rate validation.
- [Pin the host termination file and cap guest termination text](termination-file.md) — Host file-write hardening.
- [Separate host metrics from guest metrics](metrics-origin.md) — Telemetry provenance correction.
- [Remove the reachable unimplemented Firecracker metrics call](vmm-metrics-panic.md) — Inherited panic-path removal.

## VMM lifecycle and diagnostics

- [Restore useful API errors with bounded bodies and deadlines](firecracker-api-errors.md) — Restoration plus host input hardening.
- [Enable timers in the runtimes used for network namespace work](network-runtime-timers.md) — Regression introduced and repaired in the fork.
- [Stop unbounded VMM logging without closing its pipe](vmm-stderr-bounds.md) — Host diagnostics hardening.
- [Observe VMM exit independently of stderr completion](vmm-exit-observation.md) — Lifecycle correction accompanying bounded logging.
- [Do not construct jailed cleanup paths from an empty VM path](unprepared-vm-cleanup.md) — Regression prevention during fork simplification.

## Tracing

- [Restore linked tracing and a compatible host exporter](trace-integration.md) — Restoration plus integration corrections.
- [Bind trace sockets under long sandbox paths without replacing listeners](trace-socket-path.md) — Integration correction during restoration.
- [Discard every failed guest trace connection before the next batch](trace-reconnect.md) — Correction to inherited exporter error handling.
- [Bound guest trace frames and assign host-controlled exporter identity](trace-input-boundary.md) — Host telemetry hardening.

## Startup scheduling

[Network namespace thread join and startup stall](network-thread-stall.md)
explains the scheduling failure, the correction and the limits of its diagnosis.

## Attribution boundary

Hugepages, basic MMIO discovery, namespace isolation, trace-forwarder privilege
dropping, synchronous sandbox-cgroup placement and complete trace framing were
already upstream behavior. Moving those implementations is not credited as a new
fix. Vendored ttrpc is compared with its original 0.9.0 package, rather than treating
all vendored code as original hardening. Dependency/platform deletions and concrete
type refactors are not separately counted as behavioral fixes.

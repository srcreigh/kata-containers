# Do not construct jailed cleanup paths from an empty VM path

[Fix inventory](README.md) · **Regression prevention during fork simplification**

Implementation history: `fe8a41b4e9ff`.

## Before

Removing an alternate jailed/unjailed state branch made an empty `vm_path` important. A failed or never-completed prepare must not turn an empty base into an apparently valid absolute `/root/...` cleanup target.

## Change

Cleanup path construction explicitly returns no jailed cleanup target when the VM path is empty. Prepared VMs retain their concrete jailed paths. Tests distinguish unprepared/empty state from a normal jail path.

## Validation and tradeoffs

The unused-machinery report records this integration correction before build/rollout, and the native hypervisor tests cover cleanup path selection.

This was caught during review; there is no evidence here of a production `/root` deletion . The guard is not a complete audit of every cleanup path and does not authorize touching devmapper metadata.

Validation summaries describe the original development checks, not a fresh test run during publication. Deployment logs and operator scripts are outside this source repository. Source-level regressions, where present, remain alongside the implementation.

## Source

- [src/runtime-rs/crates/hypervisor/src/firecracker/fc_api.rs](../../src/runtime-rs/crates/hypervisor/src/firecracker/fc_api.rs)

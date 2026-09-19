# Apply a converted swap limit even when its value is zero

[Fix inventory](README.md) · **Correction to inherited cgroup-v2 limit handling**

Implementation history: `749e1521e801`.

## Before

OCI memory.swap represents memory plus swap; cgroup v2 needs a separate swap limit. When a positive combined limit equals the memory limit, conversion yields zero. The old `if swap != 0` condition skipped the write, potentially leaving an existing swap allowance unchanged.

## Change

`set_memory_resources` also writes when the original OCI swap value is positive: `swap != 0 || memory.swap().unwrap_or(0) > 0`. For example, memory=256 MiB and combined memory+swap=256 MiB now applies a zero swap allowance. The conversion function itself was inherited.

## Validation and tradeoffs

Source comparison identifies this condition in `749e1521e`; the rustjail native suite includes `test_memory_swap_v2_limit`, covering conversion cases.

The existing test is not a live regression proving a nonzero-to-zero kernel transition. The fork does not provision guest swap, so many current workloads will not observe a difference. This corrects the limit setter if that condition arises, rather than adding swap functionality.

Validation summaries describe the original development checks, not a fresh test run during publication. Deployment logs and operator scripts are outside this source repository. Source-level regressions, where present, remain alongside the implementation.

## Source

- [src/agent/rustjail/src/cgroups/fs/mod.rs](../../src/agent/rustjail/src/cgroups/fs/mod.rs)

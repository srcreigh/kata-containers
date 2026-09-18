# Minimal Firecracker guest agent

This fork runs the static agent as guest PID 1. Build it as part of the native Linux Cargo workspace.
The supported workload contract is in the [root README](../../README.md).

## Configuration

The complete schema lives in [`src/config.rs`](src/config.rs). Unknown `agent.*`
kernel options, unknown TOML fields and unknown `KATA_AGENT_*` environment options
are errors, including removed options set to false or zero. Ordinary kernel
arguments are tolerated. All agent kernel options are checked before loading a
config file, so a file cannot conceal an unsupported option elsewhere.

| Kernel option | TOML field | Default |
| --- | --- | --- |
| `agent.log` | `log_level` | `info` |
| `agent.hotplug_timeout` | `hotplug_timeout` | 3 seconds |
| `agent.log_vport` | `log_vport` | 0 (stdout); deployment uses 1025 |
| `agent.container_pipe_size` | `container_pipe_size` | 0 (kernel default) |
| `agent.server_addr` | `server_addr` | `vsock://-1:1024` |
| `cgroup_no_v1` | `cgroup_no_v1` | empty |
| `systemd.unified_cgroup_hierarchy` | `unified_cgroup_hierarchy` | false |
| `agent.config_file` | — | none |

Timeouts are positive whole seconds on the kernel command line, or
`{ secs = 3, nanos = 0 }` in TOML. Pipe sizes must be nonnegative. Log levels are
critical/fatal/panic, error, warning/warn, info, debug, and trace; trace is a log
level, not distributed tracing.

Precedence: defaults, kernel settings, selected TOML file, environment. A selected
file replaces kernel agent settings with its own defaults/values. `--config PATH`
or `-c PATH` selects a file instead of `agent.config_file`. The only environment
overrides are `KATA_AGENT_SERVER_ADDR` and `KATA_AGENT_LOG_LEVEL`.

Confidential computing, guest extensions, init-data, policy-engine builds, debug
shells, trace export and pass-fd IO have no implementation here. The disabled
upstream policy build variant was removed; the deployed runtime did not use it.
Built-in request validation, safe paths, namespaces, credentials and cgroups remain.
See the root README for the supported contract.

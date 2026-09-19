// Copyright (c) 2019 Ant Financial
// SPDX-License-Identifier: Apache-2.0

//! The complete supported guest configuration. Unknown agent options fail closed.
use anyhow::{bail, ensure, Context, Result};
use serde::Deserialize;
use std::{env, fs, str::FromStr, time::Duration};

#[derive(Debug)]
pub struct AgentConfig {
    pub tracing: bool,
    pub log_level: slog::Level,
    pub hotplug_timeout: Duration,
    pub log_vport: u32,
    pub container_pipe_size: i32,
    pub server_addr: String,
    pub cgroup_no_v1: String,
    pub unified_cgroup_hierarchy: bool,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigFile {
    tracing: Option<bool>,
    log_level: Option<String>,
    hotplug_timeout: Option<Duration>,
    log_vport: Option<u32>,
    container_pipe_size: Option<i32>,
    server_addr: Option<String>,
    cgroup_no_v1: Option<String>,
    unified_cgroup_hierarchy: Option<bool>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            tracing: false,
            log_level: slog::Level::Info,
            hotplug_timeout: Duration::from_secs(3),
            log_vport: 0,
            container_pipe_size: 0,
            server_addr: "vsock://-1:1024".into(),
            cgroup_no_v1: String::new(),
            unified_cgroup_hierarchy: false,
        }
    }
}

impl FromStr for AgentConfig {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        let file: ConfigFile = toml::from_str(s)?;
        let mut config = Self::default();
        if let Some(level) = file.log_level {
            config.log_level = log_level(&level)?;
        }
        macro_rules! copy {
            ($($field:ident),*) => { $(if let Some(v) = file.$field { config.$field = v; })* };
        }
        copy!(
            tracing,
            hotplug_timeout,
            log_vport,
            container_pipe_size,
            server_addr,
            cgroup_no_v1,
            unified_cgroup_hierarchy
        );
        config.validate()?;
        Ok(config)
    }
}

impl AgentConfig {
    fn validate(&self) -> Result<()> {
        ensure!(
            !self.hotplug_timeout.is_zero(),
            "hotplug_timeout must be positive"
        );
        ensure!(
            self.container_pipe_size >= 0,
            "container_pipe_size must not be negative"
        );
        ensure!(
            !self.server_addr.is_empty(),
            "server_addr must not be empty"
        );
        Ok(())
    }

    pub fn from_cmdline(file: &str, args: Vec<String>) -> Result<Self> {
        // Validate all kernel options before choosing any config file. A config-file
        // option must not hide an unsupported option appearing before or after it.
        let cmdline = fs::read_to_string(file).context("read kernel command line")?;
        let mut config = Self::default();
        let mut config_file = None;
        for param in cmdline.split_ascii_whitespace() {
            let (key, value) = param.split_once('=').unwrap_or((param, ""));
            match key {
                "agent.trace" => {
                    config.tracing = match value {
                        "" | "true" | "1" => true,
                        "false" | "0" => false,
                        _ => bail!("invalid agent.trace value"),
                    }
                }
                "agent.log" => config.log_level = log_level(value)?,
                "agent.hotplug_timeout" => {
                    config.hotplug_timeout = Duration::from_secs(value.parse()?)
                }
                "agent.log_vport" => config.log_vport = value.parse()?,
                "agent.container_pipe_size" => config.container_pipe_size = value.parse()?,
                "agent.server_addr" => {
                    ensure!(!value.is_empty(), "empty server address");
                    config.server_addr = value.into();
                }
                "agent.config_file" => {
                    ensure!(!value.is_empty(), "empty config file path");
                    config_file = Some(value.to_owned());
                }
                "cgroup_no_v1" => config.cgroup_no_v1 = value.into(),
                "systemd.unified_cgroup_hierarchy" => {
                    config.unified_cgroup_hierarchy = match value {
                        "1" | "true" => true,
                        "0" | "false" => false,
                        _ => bail!("invalid unified_cgroup_hierarchy value"),
                    }
                }
                _ if key.starts_with("agent.") => bail!("kata-fc: unsupported agent option {key}"),
                _ => {} // Ordinary Linux kernel arguments are not agent configuration.
            }
        }
        config.validate()?;
        if let Some(position) = args.iter().position(|a| a == "--config" || a == "-c") {
            config_file = Some(
                args.get(position + 1)
                    .context("missing agent config path")?
                    .clone(),
            );
        }
        if let Some(path) = config_file {
            config = fs::read_to_string(&path)
                .with_context(|| format!("read agent config {path}"))?
                .parse()?;
        }
        config.apply_env(env::vars().filter(|(k, _)| k.starts_with("KATA_AGENT_")))?;
        config.validate()?;
        Ok(config)
    }

    fn apply_env(&mut self, vars: impl Iterator<Item = (String, String)>) -> Result<()> {
        for (key, value) in vars {
            match key.as_str() {
                "KATA_AGENT_SERVER_ADDR" => self.server_addr = value,
                "KATA_AGENT_LOG_LEVEL" => self.log_level = log_level(&value)?,
                _ => bail!("kata-fc: unsupported agent environment option {key}"),
            }
        }
        self.validate()
    }
}

fn log_level(value: &str) -> Result<slog::Level> {
    Ok(match value {
        "fatal" | "panic" | "critical" => slog::Level::Critical,
        "error" => slog::Level::Error,
        "warn" | "warning" => slog::Level::Warning,
        "info" => slog::Level::Info,
        "debug" => slog::Level::Debug,
        "trace" => slog::Level::Trace,
        _ => bail!("invalid log level"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_kernel(contents: &str, args: Vec<String>) -> Result<AgentConfig> {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        fs::write(tmp.path(), contents).unwrap();
        AgentConfig::from_cmdline(tmp.path().to_str().unwrap(), args)
    }

    #[test]
    fn tracing_can_be_enabled_explicitly() {
        assert!(!AgentConfig::default().tracing);
        assert!(parse_kernel("agent.trace=true", vec![]).unwrap().tracing);
        assert!("tracing = true".parse::<AgentConfig>().unwrap().tracing);
        assert!(!parse_kernel("agent.trace=false", vec![]).unwrap().tracing);
        assert!(parse_kernel("agent.trace=invalid", vec![]).is_err());
    }

    #[test]
    fn supported_boot_config() {
        let config = parse_kernel("console=ttyS0 root=/dev/vda agent.log_vport=1025 systemd.unified_cgroup_hierarchy=1 cgroup_no_v1=all agent.log=debug agent.hotplug_timeout=9 agent.container_pipe_size=4096", vec![]).unwrap();
        assert_eq!(config.log_vport, 1025);
        assert_eq!(config.log_level, slog::Level::Debug);
        assert_eq!(config.hotplug_timeout, Duration::from_secs(9));
        assert_eq!(config.container_pipe_size, 4096);
        assert!(config.unified_cgroup_hierarchy);
        assert_eq!(config.cgroup_no_v1, "all");
        assert_eq!(config.server_addr, "vsock://-1:1024");
    }

    #[test]
    fn removed_options_fail_even_when_disabled_or_shadowed() {
        let file = tempfile::NamedTempFile::new().unwrap();
        fs::write(file.path(), "log_level = 'info'").unwrap();
        for key in [
            "debug_console",
            "devmode",
            "passfd_listener_port",
            "cdh_api_timeout",
            "image_pull_timeout",
            "launch_process_timeout",
            "guest_components_procs",
            "guest_components_rest_api",
            "secure_storage_integrity",
            "https_proxy",
            "no_proxy",
            "cdi_timeout",
            "visible_cdi_devices",
            "mem_agent_enable",
            "unknown",
        ] {
            for suffix in ["", "=0", "=false"] {
                let option = format!("agent.{key}{suffix}");
                for contents in [
                    option.clone(),
                    format!("agent.config_file={} {option}", file.path().display()),
                    format!("{option} agent.config_file={}", file.path().display()),
                ] {
                    assert!(parse_kernel(&contents, vec![]).is_err(), "{contents}");
                    assert!(parse_kernel(
                        &contents,
                        vec!["--config".into(), file.path().display().to_string()]
                    )
                    .is_err());
                }
            }
            assert!(format!("{key} = false").parse::<AgentConfig>().is_err());
        }
        for key in ["dev_mode", "policy_file", "debug_console_vport"] {
            assert!(format!("{key} = false").parse::<AgentConfig>().is_err());
        }
    }

    #[test]
    fn file_and_environment_precedence() {
        let file = tempfile::NamedTempFile::new().unwrap();
        fs::write(
            file.path(),
            "log_level = 'error'\ncontainer_pipe_size = 8192",
        )
        .unwrap();
        let mut config = parse_kernel(
            "agent.log=debug",
            vec!["-c".into(), file.path().display().to_string()],
        )
        .unwrap();
        assert_eq!(config.log_level, slog::Level::Error);
        assert_eq!(config.container_pipe_size, 8192);
        config
            .apply_env(
                vec![
                    ("KATA_AGENT_LOG_LEVEL".into(), "trace".into()),
                    (
                        "KATA_AGENT_SERVER_ADDR".into(),
                        "unix:///tmp/agent.sock".into(),
                    ),
                ]
                .into_iter(),
            )
            .unwrap();
        assert_eq!(config.log_level, slog::Level::Trace);
        assert_eq!(config.server_addr, "unix:///tmp/agent.sock");
        for key in [
            "KATA_AGENT_TRACING",
            "KATA_AGENT_POLICY_FILE",
            "KATA_AGENT_UNKNOWN",
        ] {
            assert!(config
                .apply_env(vec![(key.into(), "0".into())].into_iter())
                .is_err());
        }
    }

    #[test]
    fn malformed_configuration_fails() {
        for option in [
            "agent.log=bad",
            "agent.log_vport=-1",
            "agent.hotplug_timeout=0",
            "agent.hotplug_timeout=-1",
            "agent.container_pipe_size=-1",
            "agent.server_addr=",
            "agent.config_file=",
        ] {
            assert!(parse_kernel(option, vec![]).is_err(), "{option}");
        }
        assert!(parse_kernel("", vec!["--config".into()]).is_err());
        for contents in [
            "log_level='bad'",
            "log_vport=-1",
            "container_pipe_size=-1",
            "hotplug_timeout={secs=0,nanos=0}",
            "server_addr=''",
        ] {
            assert!(contents.parse::<AgentConfig>().is_err(), "{contents}");
        }
    }
}

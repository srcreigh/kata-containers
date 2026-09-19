// Copyright (c) 2019-2022 Alibaba Cloud
// Copyright (c) 2019-2022 Ant Group
//
// SPDX-License-Identifier: Apache-2.0
//

#[macro_use]
extern crate slog;

logging::logger_with_subsystem!(sl, "virt-container");

mod container_manager;
pub mod contract;
pub mod health_check;
mod oom;
pub mod sandbox;
pub mod sandbox_persist;
mod termination;

use std::sync::Arc;

use agent::{kata::KataAgent, AGENT_KATA};
use anyhow::{anyhow, Context, Result};
use common::{message::Message, types::SandboxConfig, RuntimeInstance};
use hypervisor::Hypervisor;
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
use hypervisor::{firecracker::Firecracker, HYPERVISOR_FIRECRACKER};

#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
use kata_types::config::FirecrackerConfig;

use kata_types::config::{hypervisor::register_hypervisor_plugin, TomlConfig};

use resource::ResourceManager;
use tokio::sync::mpsc::Sender;
use tracing::instrument;

#[derive(Debug)]
pub struct VirtContainer {}

impl VirtContainer {
    pub fn init() -> Result<()> {
        // Before start logging with virt-container, regist it
        logging::register_subsystem_logger("runtimes", "virt-container");

        // register

        #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
        {
            let firecracker_config = Arc::new(FirecrackerConfig::new());
            register_hypervisor_plugin("firecracker", firecracker_config);
        }

        Ok(())
    }

    #[instrument]
    pub async fn new_instance(
        sid: &str,
        msg_sender: Sender<Message>,
        config: Arc<TomlConfig>,
        sandbox_config: SandboxConfig,
    ) -> Result<RuntimeInstance> {
        crate::contract::validate(&config)?;
        let hypervisor = new_hypervisor(&config).await.context("new hypervisor")?;
        let agent = new_agent(&config).context("new agent")? as Arc<dyn agent::Agent>;

        let resource_manager =
            Arc::new(ResourceManager::new(sid, agent.clone(), hypervisor.clone(), config).await?);
        let pid = std::process::id();

        let sandbox = sandbox::VirtSandbox::new(
            sid,
            msg_sender,
            agent.clone(),
            hypervisor.clone(),
            resource_manager.clone(),
            sandbox_config,
        )
        .await
        .context("new virt sandbox")?;
        let container_manager = container_manager::VirtContainerManager::new(
            sid,
            pid,
            agent,
            hypervisor,
            resource_manager,
            sandbox.oom_registry.clone(),
        );
        Ok(RuntimeInstance {
            sandbox: Arc::new(sandbox),
            container_manager: Arc::new(container_manager),
        })
    }
}

async fn new_hypervisor(toml_config: &TomlConfig) -> Result<Arc<dyn Hypervisor>> {
    let hypervisor_name = &toml_config.runtime.hypervisor_name;
    let hypervisor_config = toml_config
        .hypervisor
        .get(hypervisor_name)
        .ok_or_else(|| anyhow!("failed to get hypervisor for {}", &hypervisor_name))
        .context("get hypervisor")?;

    // TODO: support other hypervisor
    // issue: https://github.com/kata-containers/kata-containers/issues/4634
    match hypervisor_name.as_str() {
        #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
        HYPERVISOR_FIRECRACKER => {
            let hypervisor = Firecracker::new();
            hypervisor
                .set_hypervisor_config(hypervisor_config.clone())
                .await;
            Ok(Arc::new(hypervisor))
        }

        _ => Err(anyhow!("Unsupported hypervisor {}", &hypervisor_name)),
    }
}

fn new_agent(toml_config: &TomlConfig) -> Result<Arc<KataAgent>> {
    let agent_name = &toml_config.runtime.agent_name;
    let agent_config = toml_config
        .agent
        .get(agent_name)
        .ok_or_else(|| anyhow!("failed to get agent for {}", &agent_name))
        .context("get agent")?;
    match agent_name.as_str() {
        AGENT_KATA => {
            let agent = KataAgent::new(agent_config.clone());
            Ok(Arc::new(agent))
        }
        _ => Err(anyhow!("Unsupported agent {}", &agent_name)),
    }
}

#[cfg(test)]
mod test {

    use super::*;

    fn default_toml_config_agent() -> Result<TomlConfig> {
        let config_content = r#"
[agent.kata]
container_pipe_size=1

[runtime]
agent_name="kata"
        "#;
        TomlConfig::load(config_content).map_err(|e| anyhow!("can not load config toml: {}", e))
    }

    #[test]
    fn test_new_agent() {
        let toml_config = default_toml_config_agent().unwrap();

        let res = new_agent(&toml_config);
        assert!(res.is_ok());
    }

    #[tokio::test]
    async fn test_new_hypervisor() {
        VirtContainer::init().unwrap();

        let toml_config = {
            let config_content = r#"
[hypervisor.firecracker]
path = "/bin/echo"
kernel = "/bin/echo"
image = "/bin/echo"
firmware = ""

[runtime]
hypervisor_name="firecracker"
"#;
            TomlConfig::load(config_content).map_err(|e| anyhow!("can not load config toml: {}", e))
        }
        .unwrap();

        let res = new_hypervisor(&toml_config).await;
        assert!(res.is_ok());
    }
}

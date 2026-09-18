// Copyright (c) 2019-2022 Alibaba Cloud
// Copyright (c) 2019-2022 Ant Group
//
// SPDX-License-Identifier: Apache-2.0
//

use std::sync::Arc;

mod endpoint;
pub use endpoint::endpoint_persist::EndpointState;
pub use endpoint::Endpoint;
mod network_entity;
mod network_info;
pub use network_info::NetworkInfo;
mod network_model;
pub use network_model::NetworkModel;
mod network_with_netns;
pub(crate) use network_with_netns::netns_has_interfaces;
pub use network_with_netns::NetworkWithNetNsConfig;
use network_with_netns::NetworkWithNetns;
mod network_pair;
use network_pair::NetworkPair;
mod utils;
pub use kata_sys_util::netns::{generate_netns_name, NetnsGuard};
use tokio::sync::RwLock;

use anyhow::{Context, Result};
use async_trait::async_trait;
use hypervisor::{device::device_manager::DeviceManager, Hypervisor};

#[derive(Debug)]
pub enum NetworkConfig {
    NetNs(NetworkWithNetNsConfig),
}

#[async_trait]
pub trait Network: Send + Sync {
    async fn setup(&self) -> Result<()>;
    async fn interfaces(&self) -> Result<Vec<agent::Interface>>;
    async fn routes(&self) -> Result<Vec<agent::Route>>;
    async fn neighs(&self) -> Result<Vec<agent::ARPNeighbor>>;
    async fn save(&self) -> Option<Vec<EndpointState>>;
    async fn remove(&self, h: &dyn Hypervisor) -> Result<()>;
    /// Returns the list of network endpoints. Used to resolve PCI paths
    /// via QMP before sending update_interface to the agent.
    async fn endpoints(&self) -> Vec<std::sync::Arc<dyn endpoint::Endpoint>> {
        vec![]
    }
}

pub async fn new(
    config: &NetworkConfig,
    d: Arc<RwLock<DeviceManager>>,
) -> Result<Arc<dyn Network>> {
    match config {
        NetworkConfig::NetNs(c) => Ok(Arc::new(
            NetworkWithNetns::new(c, d)
                .await
                .context("new network with netns")?,
        )),
    }
}

/// DAN bypasses the supported tcfilter namespace setup. Reject its configuration
/// before parsing it, opening device sockets, or configuring a host interface.
pub fn reject_dan(config: &kata_types::config::TomlConfig, sid: &str) -> Result<()> {
    let path = std::path::Path::new(&config.runtime.dan_conf).join(format!("{sid}.json"));
    anyhow::ensure!(
        !path.try_exists()?,
        "kata-fc: directly attachable networking is unsupported"
    );
    Ok(())
}

#[cfg(test)]
mod minimal_tests {
    #[test]
    fn rejects_dan_before_parsing_or_network_setup() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = kata_types::config::TomlConfig::default();
        config.runtime.dan_conf = dir.path().display().to_string();
        super::reject_dan(&config, "sandbox").unwrap();
        std::fs::write(dir.path().join("sandbox.json"), "invalid json").unwrap();
        assert!(super::reject_dan(&config, "sandbox")
            .unwrap_err()
            .to_string()
            .contains("directly attachable networking is unsupported"));
    }
}

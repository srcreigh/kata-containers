//Copyright (c) 2019-2022 Alibaba Cloud
//Copyright (c) 2023 Nubificus Ltd
//
//SPDX-License-Identifier: Apache-2.0

mod fc_api;
mod inner;
mod inner_device;
mod inner_hypervisor;
mod mac;

use super::HypervisorState;
use crate::{device::DeviceType, Hypervisor, HypervisorConfig};
use anyhow::Context;
use anyhow::Result;
use async_trait::async_trait;
use inner::FcInner;
use persist::sandbox_persist::Persist;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::sync::RwLock;

#[derive(Debug)]
pub struct Firecracker {
    inner: Arc<RwLock<FcInner>>,
    exit_code: Mutex<i32>,
}

// Convenience function to set the scope.
pub fn sl() -> slog::Logger {
    slog_scope::logger().new(o!("subsystem" => "firecracker"))
}

impl Default for Firecracker {
    fn default() -> Self {
        Self::new()
    }
}

impl Firecracker {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(FcInner::new())),
            exit_code: Mutex::new(0),
        }
    }

    pub async fn set_hypervisor_config(&self, config: HypervisorConfig) {
        let mut inner = self.inner.write().await;
        inner.set_hypervisor_config(config)
    }
}

#[async_trait]
impl Hypervisor for Firecracker {
    async fn prepare_vm(
        &self,
        id: &str,
        netns: Option<String>,
        _annotations: &HashMap<String, String>,
        selinux_label: Option<String>,
    ) -> Result<()> {
        let mut inner = self.inner.write().await;
        inner.prepare_vm(id, netns, selinux_label).await
    }

    async fn start_vm(&self, timeout: i32) -> Result<()> {
        let mut inner = self.inner.write().await;
        inner.start_vm(timeout).await
    }

    async fn stop_vm(&self) -> Result<()> {
        let mut inner = self.inner.write().await;
        inner.stop_vm().await
    }

    async fn wait_vm(&self) -> Result<i32> {
        debug!(sl(), "Wait fc sandbox");
        let mut waiter = self.exit_code.lock().await;

        loop {
            {
                let inner = self.inner.read().await;
                match inner.wait_vm().await {
                    Ok(Some(code)) => {
                        *waiter = code;
                        return Ok(code);
                    }
                    Ok(None) => {}
                    Err(_) => return Ok(*waiter),
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }

    async fn add_device(&self, device: DeviceType) -> Result<()> {
        self.inner.write().await.add_device(device).await
    }

    async fn get_agent_socket(&self) -> Result<String> {
        let inner = self.inner.read().await;
        inner.get_agent_socket().await
    }

    async fn hypervisor_config(&self) -> HypervisorConfig {
        let inner = self.inner.read().await;
        inner.hypervisor_config()
    }

    async fn cleanup(&self) -> Result<()> {
        let inner = self.inner.read().await;
        inner.cleanup().await
    }

    async fn get_vmm_master_tid(&self) -> Result<u32> {
        let inner = self.inner.read().await;
        inner.get_vmm_master_tid().await
    }

    async fn save_state(&self) -> Result<HypervisorState> {
        self.save().await
    }
}
#[async_trait]
impl Persist for Firecracker {
    type State = HypervisorState;
    type ConstructorArgs = ();
    /// Save a state of the component.
    async fn save(&self) -> Result<Self::State> {
        let inner = self.inner.read().await;
        inner.save().await.context("save hypervisor state")
    }
    /// Restore a component from a specified state.
    async fn restore(
        _hypervisor_args: Self::ConstructorArgs,
        hypervisor_state: Self::State,
    ) -> Result<Self> {
        let inner = FcInner::restore((), hypervisor_state).await?;

        Ok(Self {
            inner: Arc::new(RwLock::new(inner)),
            exit_code: Mutex::new(0),
        })
    }
}

// Copyright (c) 2019-2022 Alibaba Cloud
// Copyright (c) 2019-2022 Ant Group
//
// SPDX-License-Identifier: Apache-2.0
//

use anyhow::{Context, Result};

use async_trait::async_trait;

use crate::{
    device::{Device, DeviceType},
    Hypervisor as hypervisor,
};

// This is the first usable vsock context ID. All the vsocks
// can use the same ID, since it's only used in the guest.
pub const DEFAULT_GUEST_VSOCK_CID: u32 = 0x3;

#[derive(Clone, Debug, Default)]
pub struct HybridVsockConfig {
    /// A 32-bit Context Identifier (CID) used to identify the guest.
    pub guest_cid: u32,

    /// unix domain socket path
    pub uds_path: String,
}

#[derive(Clone, Debug, Default)]
pub struct HybridVsockDevice {
    /// Unique identifier of the device
    pub id: String,

    /// config information for HybridVsockDevice
    pub config: HybridVsockConfig,
}

impl HybridVsockDevice {
    pub fn new(device_id: &String, config: &HybridVsockConfig) -> Self {
        Self {
            id: format!("vsock-{device_id}"),
            config: config.clone(),
        }
    }
}

#[async_trait]
impl Device for HybridVsockDevice {
    async fn attach(&mut self, h: &dyn hypervisor) -> Result<()> {
        h.add_device(DeviceType::HybridVsock(self.clone()))
            .await
            .context("add hybrid vsock device.")?;

        return Ok(());
    }

    async fn detach(&mut self, _h: &dyn hypervisor) -> Result<Option<u64>> {
        // no need to do detach, just return Ok(None)
        Ok(None)
    }

    async fn update(&mut self, _h: &dyn hypervisor) -> Result<()> {
        // There's no need to do update for hvsock device
        Ok(())
    }

    async fn get_device_info(&self) -> DeviceType {
        DeviceType::HybridVsock(self.clone())
    }

    async fn increase_attach_count(&mut self) -> Result<bool> {
        // hybrid vsock devices will not be attached multiple times, Just return Ok(false)

        Ok(false)
    }

    async fn decrease_attach_count(&mut self) -> Result<bool> {
        // hybrid vsock devices will not be detached multiple times, Just return Ok(false)

        Ok(false)
    }
}

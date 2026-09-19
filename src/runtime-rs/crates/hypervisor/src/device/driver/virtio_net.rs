// Copyright (c) 2019-2022 Alibaba Cloud
// Copyright (c) 2019-2022 Ant Group
//
// SPDX-License-Identifier: Apache-2.0
//

use std::fmt;

use anyhow::{Context, Result};
use async_trait::async_trait;

use crate::device::{Device, DeviceType};
use crate::Hypervisor as hypervisor;

#[derive(Clone, Default)]
pub struct Address(pub [u8; 6]);

impl fmt::Debug for Address {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let b = self.0;
        write!(
            f,
            "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            b[0], b[1], b[2], b[3], b[4], b[5]
        )
    }
}

#[derive(Clone, Debug, Default)]
pub struct NetworkConfig {
    /// Host TAP interface opened by Firecracker.
    pub host_dev_name: String,
    /// MAC address presented to the guest.
    pub guest_mac: Option<Address>,
}

#[derive(Clone, Debug, Default)]
pub struct NetworkDevice {
    /// Unique identifier of the device
    pub device_id: String,

    /// Network Device config info
    pub config: NetworkConfig,
}

impl NetworkDevice {
    // new creates a NetworkDevice
    pub fn new(device_id: String, config: &NetworkConfig) -> Self {
        Self {
            device_id,
            config: config.clone(),
        }
    }
}

#[async_trait]
impl Device for NetworkDevice {
    async fn attach(&mut self, h: &dyn hypervisor) -> Result<()> {
        h.add_device(DeviceType::Network(self.clone()))
            .await
            .context("add network device.")?;

        Ok(())
    }

    async fn detach(&mut self) -> Result<Option<u64>> {
        // Network cleanup removes tc filters; it never releases a block-drive slot.
        Ok(None)
    }

    async fn get_device_info(&self) -> DeviceType {
        DeviceType::Network(self.clone())
    }
}

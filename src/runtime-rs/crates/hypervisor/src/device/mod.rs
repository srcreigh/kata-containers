// Copyright (c) 2019-2023 Alibaba Cloud
// Copyright (c) 2019-2023 Ant Group
//
// SPDX-License-Identifier: Apache-2.0
//

use std::fmt;
use std::sync::Arc;

use tokio::sync::Mutex;

use crate::device::driver::virtio_blk_modern::BlockDeviceModern;
use crate::{BlockConfigModern, Hypervisor as hypervisor, NetworkConfig, NetworkDevice};
use anyhow::Result;
use async_trait::async_trait;

pub mod device_manager;
pub mod driver;
pub mod util;

#[derive(Debug)]
pub enum DeviceConfig {
    BlockCfgModern(BlockConfigModern),
    NetworkCfg(NetworkConfig),
}

#[derive(Debug, Clone)]
pub enum DeviceType {
    Network(NetworkDevice),
    BlockModern(Arc<Mutex<BlockDeviceModern>>),
}

impl fmt::Display for DeviceType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

#[async_trait]
pub trait Device: std::fmt::Debug + Send + Sync {
    // attach is to plug device into VM
    async fn attach(&mut self, h: &dyn hypervisor) -> Result<()>;
    // detach is to unplug device from VM
    async fn detach(&mut self) -> Result<Option<u64>>;
    // get_device_info returns device config
    async fn get_device_info(&self) -> DeviceType;
}

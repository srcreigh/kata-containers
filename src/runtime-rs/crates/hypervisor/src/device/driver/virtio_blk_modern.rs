// Copyright (c) 2026 Ant Group
//
// SPDX-License-Identifier: Apache-2.0
//

use std::sync::Arc;
use tokio::sync::Mutex;

use crate::device::util::do_decrease_count;
use crate::device::util::do_increase_count;
use crate::device::Device;
use crate::device::DeviceType;
use crate::Hypervisor as hypervisor;
use anyhow::{Context, Result};
use async_trait::async_trait;

pub const VIRTIO_BLOCK_MMIO: &str = "virtio-blk-mmio";

/// Firecracker raw block disk. Alternate transports and QEMU disk formats are
/// deliberately absent; driver_option is validated before device allocation.
#[derive(Debug, Clone, Default)]
pub struct BlockConfigModern {
    pub path_on_host: String,
    pub is_readonly: bool,
    pub index: u64,
    pub driver_option: String,
    pub virt_path: String,
    pub major: i64,
    pub minor: i64,
}

#[derive(Debug, Clone, Default)]
pub struct BlockDeviceModern {
    pub device_id: String,
    pub attach_count: u64,
    pub config: BlockConfigModern,
}

#[derive(Debug, Clone)]
pub struct BlockDeviceModernHandle {
    inner: Arc<Mutex<BlockDeviceModern>>,
}

impl BlockDeviceModernHandle {
    pub fn new(device_id: String, config: BlockConfigModern) -> Self {
        Self {
            inner: Arc::new(Mutex::new(BlockDeviceModern {
                device_id,
                attach_count: 0,
                config,
            })),
        }
    }

    pub fn arc(&self) -> Arc<Mutex<BlockDeviceModern>> {
        self.inner.clone()
    }

    pub async fn snapshot_config(&self) -> BlockConfigModern {
        self.inner.lock().await.config.clone()
    }

    pub async fn device_id(&self) -> String {
        self.inner.lock().await.device_id.clone()
    }

    pub async fn attach_count(&self) -> u64 {
        self.inner.lock().await.attach_count
    }
}

#[async_trait]
impl Device for BlockDeviceModernHandle {
    async fn attach(&mut self, h: &dyn hypervisor) -> Result<()> {
        // increase attach count, skip attach the device if the device is already attached
        if self
            .increase_attach_count()
            .await
            .context("failed to increase attach count")?
        {
            return Ok(());
        }

        if let Err(e) = h.add_device(DeviceType::BlockModern(self.arc())).await {
            error!(sl!(), "failed to attach block device: {:?}", e);
            self.decrease_attach_count().await?;

            return Err(e);
        }

        Ok(())
    }

    async fn detach(&mut self) -> Result<Option<u64>> {
        // Keep the drive slot while another attachment still references it.
        if self
            .decrease_attach_count()
            .await
            .context("failed to decrease attach count")?
        {
            return Ok(None);
        }
        Ok(Some(self.snapshot_config().await.index))
    }

    async fn get_device_info(&self) -> DeviceType {
        DeviceType::BlockModern(self.inner.clone())
    }
}

impl BlockDeviceModernHandle {
    async fn increase_attach_count(&mut self) -> Result<bool> {
        let mut guard = self.inner.lock().await;
        do_increase_count(&mut guard.attach_count)
    }

    async fn decrease_attach_count(&mut self) -> Result<bool> {
        let mut guard = self.inner.lock().await;
        do_decrease_count(&mut guard.attach_count)
    }
}

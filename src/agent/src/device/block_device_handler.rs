// Copyright (c) 2019 Ant Financial
// Copyright (c) 2024 Intel Corporation
//
// SPDX-License-Identifier: Apache-2.0
//

use crate::device::{DeviceContext, DeviceHandler, DeviceInfo, SpecUpdate};
use anyhow::{anyhow, Context, Result};
use kata_types::device::DRIVER_BLK_MMIO_TYPE;
use protocols::agent::Device;
use std::path::Path;

#[derive(Debug)]
pub struct VirtioBlkMmioDeviceHandler {}

#[async_trait::async_trait]
impl DeviceHandler for VirtioBlkMmioDeviceHandler {
    #[tracing::instrument(skip_all)]
    fn driver_types(&self) -> &[&str] {
        &[DRIVER_BLK_MMIO_TYPE]
    }

    #[tracing::instrument(skip_all)]
    async fn device_handler(&self, device: &Device, ctx: &mut DeviceContext) -> Result<SpecUpdate> {
        if device.vm_path.is_empty() {
            return Err(anyhow!("Invalid path for virtio mmio blk device"));
        }
        if !Path::new(&device.vm_path).exists() {
            get_virtio_blk_mmio_device_name(ctx.sandbox, &device.vm_path.to_string())
                .await
                .context("failed to get mmio device name")?;
        }

        let info = DeviceInfo::new(device.vm_path(), true).context("New device info")?;
        anyhow::ensure!(
            info.cgroup_type == "b",
            "MMIO mapping requires a block device"
        );
        Ok(info.into())
    }
}

use crate::storage::mmio::get_virtio_blk_mmio_device_name;

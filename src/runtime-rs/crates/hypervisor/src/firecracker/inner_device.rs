//Copyright (c) 2019-2022 Alibaba Cloud
//Copyright (c) 2019-2022 Ant Group
//Copyright (c) 2023 Nubificus Ltd
//
//SPDX-License-Identifier: Apache-2.0

use super::FcInner;
use crate::firecracker::sl;
use crate::{device::DeviceType, VmmState};
use anyhow::{Context, Result};

impl FcInner {
    pub(crate) async fn add_device(&mut self, device: DeviceType) -> Result<()> {
        anyhow::ensure!(
            self.state != VmmState::NotReady,
            "kata-fc: cannot attach a device before preparing the VMM"
        );

        debug!(sl(), "Add Device {} ", &device);

        match device {
            DeviceType::BlockModern(block_mod) => {
                let block = block_mod.lock().await.clone();
                self.hotplug_block_device(block.config.path_on_host.as_str(), block.config.index)
                    .await
                    .context("add block device")
            }
            DeviceType::Network(network) => self
                .add_net_device(&network.config, network.device_id)
                .await
                .context("add net device"),
        }
    }

    // Since Firecracker doesn't support sharefs, we patch block devices on pre-start inserted
    // dummy drives
    pub(crate) async fn hotplug_block_device(&mut self, path: &str, id: u64) -> Result<()> {
        if id > 0 {
            self.patch_container_rootfs(&id.to_string(), path).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn device_attachment_requires_prepared_vmm() {
        let mut fc = FcInner::new();
        let error = fc
            .add_device(DeviceType::Network(Default::default()))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("before preparing the VMM"));
        assert!(fc.pid.is_none());
    }
}

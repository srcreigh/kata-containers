// Copyright (c) 2019-2022 Alibaba Cloud
// Copyright (c) 2019-2022 Ant Group
//
// SPDX-License-Identifier: Apache-2.0
//

use super::{Rootfs, ROOTFS};
use crate::{block_device::agent_storage_source_from_block_config, guest_paths::do_get_guest_path};
use agent::Storage;
use anyhow::{Context, Result};
use async_trait::async_trait;
use hypervisor::{
    device::{
        device_manager::{do_handle_device, get_block_device_info, DeviceManager},
        DeviceConfig, DeviceType,
    },
    BlockConfigModern,
};
use kata_types::fs::VM_ROOTFS_FILESYSTEM_XFS;
use kata_types::mount::Mount;
use nix::sys::stat::{self, SFlag};
use tokio::sync::RwLock;

const BLOCKFILE_ROOTFS_FLAG: &str = "loop";

pub(crate) struct BlockRootfs {
    guest_path: String,
    device_id: String,
    storage: agent::Storage,
}

impl BlockRootfs {
    pub async fn new(
        d: &RwLock<DeviceManager>,
        _sid: &str,
        cid: &str,
        dev_id: u64,
        rootfs: &Mount,
    ) -> Result<Self> {
        let container_path = do_get_guest_path(ROOTFS, cid, false);
        let blkdev_info = get_block_device_info(d).await;
        let block_driver = blkdev_info.block_device_driver.clone();
        let block_device_config = &mut BlockConfigModern {
            major: stat::major(dev_id) as i64,
            minor: stat::minor(dev_id) as i64,
            driver_option: block_driver.clone(),
            path_on_host: rootfs.source.clone(),
            ..Default::default()
        };

        // create and insert block device into Kata VM
        let device_info = do_handle_device(
            d,
            &DeviceConfig::BlockCfgModern(block_device_config.clone()),
        )
        .await
        .context("do handle device failed.")?;

        let mut storage = Storage {
            fs_type: rootfs.fs_type.clone(),
            mount_point: container_path.clone(),
            options: rootfs.options.clone(),
            ..Default::default()
        };

        // XFS rootfs: add 'nouuid' to avoid UUID conflicts when the same
        // disk image is mounted across multiple VMs/containers.
        // This allows mounting XFS volumes that share the same UUID.
        if rootfs.fs_type == VM_ROOTFS_FILESYSTEM_XFS {
            // Add nouuid to existing options if not already present
            if !storage.options.iter().any(|opt| opt == "nouuid") {
                storage.options.push("nouuid".to_string());
            }
        }

        let mut device_id: String = "".to_owned();
        if let DeviceType::BlockModern(device_mod) = device_info {
            let device = device_mod.lock().await.clone();
            storage.driver = device.config.driver_option.clone();
            storage.source = agent_storage_source_from_block_config(&device.config)?;
            device_id = device.device_id;
        }

        Ok(Self {
            guest_path: container_path.clone(),
            device_id,
            storage,
        })
    }
}

#[async_trait]
impl Rootfs for BlockRootfs {
    async fn get_guest_rootfs_path(&self) -> Result<String> {
        Ok(self.guest_path.clone())
    }

    async fn get_storage(&self) -> Vec<Storage> {
        vec![self.storage.clone()]
    }

    async fn cleanup(&self, device_manager: &RwLock<DeviceManager>) -> Result<()> {
        device_manager
            .write()
            .await
            .try_remove_device(&self.device_id)
            .await
    }
}

pub(crate) fn is_block_rootfs(m: &Mount) -> Option<(u64, Mount)> {
    if m.source.is_empty() {
        return None;
    }

    match stat::stat(m.source.as_str()) {
        Ok(fstat) => {
            if SFlag::from_bits_truncate(fstat.st_mode) == SFlag::S_IFBLK {
                let dev_id = fstat.st_rdev;
                let mut volume = m.clone();

                //clear the volume resource thus the block device will use the dev_id
                //to find the device's host path;
                volume.source = String::new();
                return Some((dev_id, volume));
            }

            if SFlag::from_bits_truncate(fstat.st_mode) == SFlag::S_IFREG
                && m.options.contains(&BLOCKFILE_ROOTFS_FLAG.to_string())
            {
                //use the block file's inode as the device id, which can make sure it's unique.
                let dev_id = fstat.st_ino;
                let options = m
                    .options
                    .clone()
                    .into_iter()
                    .filter(|o| !o.eq(BLOCKFILE_ROOTFS_FLAG))
                    .collect();

                //discard the blockfile rootfs's mount option "loop"
                let mut volume = m.clone();
                volume.options = options;

                return Some((dev_id, volume));
            }
        }
        Err(_) => return None,
    };
    None
}

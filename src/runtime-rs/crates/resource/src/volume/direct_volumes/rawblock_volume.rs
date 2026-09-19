// Copyright (c) 2023 Alibaba Cloud
// Copyright (c) 2023 Ant Group
//
// SPDX-License-Identifier: Apache-2.0
//

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use hypervisor::{
    device::{
        device_manager::{do_handle_device, get_block_device_info, DeviceManager},
        DeviceConfig,
    },
    BlockConfigModern,
};
use kata_types::mount::DirectVolumeMountInfo;
use nix::sys::{stat, stat::SFlag};
use oci_spec::runtime as oci;
use tokio::sync::RwLock;

use crate::volume::{
    direct_volumes::KATA_DIRECT_VOLUME_TYPE,
    utils::{handle_block_volume, is_block_device_readonly},
    Volume,
};

#[derive(Clone)]
pub(crate) struct RawblockVolume {
    storage: agent::Storage,
    mount: oci::Mount,
    device_id: String,
}

/// RawblockVolume for raw block volume
impl RawblockVolume {
    pub(crate) async fn new(
        d: &RwLock<DeviceManager>,
        m: &oci::Mount,
        mount_info: &DirectVolumeMountInfo,
        read_only: bool,
        sid: &str,
    ) -> Result<Self> {
        let fs_group = supported_metadata(&mount_info.metadata)?;
        let blkdev_info = get_block_device_info(d).await;

        // check volume type
        if !supported_volume_type(&mount_info.volume_type) {
            return Err(anyhow!(
                "volume type {:?} is invalid",
                mount_info.volume_type
            ));
        }

        let fstat = stat::stat(mount_info.device.as_str())
            .with_context(|| format!("stat volume device file: {}", mount_info.device.clone()))?;
        if SFlag::from_bits_truncate(fstat.st_mode) != SFlag::S_IFREG
            && SFlag::from_bits_truncate(fstat.st_mode) != SFlag::S_IFBLK
        {
            return Err(anyhow!(
                "invalid volume device {:?} for volume type {:?}",
                mount_info.device,
                mount_info.volume_type
            ));
        }

        // For a real block device, honor its host read-only flag (BLKROGET) in
        // addition to the mount-derived intent, so a device marked read-only on
        // the host is exposed read-only to the guest. (Not applicable to
        // regular-file backed images.)
        let read_only = read_only
            || (SFlag::from_bits_truncate(fstat.st_mode) == SFlag::S_IFBLK
                && is_block_device_readonly(mount_info.device.as_str()).unwrap_or_else(|e| {
                    warn!(
                        sl!(),
                        "could not query block device read-only flag for {}: {:?}",
                        mount_info.device,
                        e
                    );
                    false
                }));

        let block_config = BlockConfigModern {
            path_on_host: mount_info.device.clone(),
            is_readonly: read_only,
            driver_option: blkdev_info.block_device_driver,
            ..Default::default()
        };

        // create and insert block device into Kata VM
        let device_info = do_handle_device(d, &DeviceConfig::BlockCfgModern(block_config.clone()))
            .await
            .context("do handle device failed.")?;

        let mut block_volume = handle_block_volume(
            device_info,
            m,
            read_only,
            sid,
            &mount_info.fs_type,
            Some(&mount_info.options),
        )
        .await
        .context("do handle block volume failed")?;

        block_volume.0.fs_group = fs_group;

        Ok(Self {
            storage: block_volume.0,
            mount: block_volume.1,
            device_id: block_volume.2,
        })
    }
}

#[async_trait]
impl Volume for RawblockVolume {
    fn get_volume_mount(&self) -> Result<Vec<oci::Mount>> {
        Ok(vec![self.mount.clone()])
    }

    fn get_storage(&self) -> Result<Vec<agent::Storage>> {
        Ok(vec![self.storage.clone()])
    }

    async fn cleanup(&self, device_manager: &RwLock<DeviceManager>) -> Result<()> {
        device_manager
            .write()
            .await
            .try_remove_device(&self.device_id)
            .await
    }
}

// "blk" is the format emitted by kata-runtime direct-volume and compatible CSI drivers.
// Retain the Rust-native spelling for callers already using it; never default unknown types.
fn supported_volume_type(value: &str) -> bool {
    matches!(value, "blk" | KATA_DIRECT_VOLUME_TYPE)
}
#[cfg(test)]
mod minimal_tests {
    use super::*;
    #[test]
    fn direct_volume_types_fail_closed() {
        for value in ["blk", "directvol"] {
            assert!(supported_volume_type(value));
        }
        for value in ["", "spdkvol", "spoolvol", "vfiovol", "anything"] {
            assert!(!supported_volume_type(value));
        }
    }
}

fn supported_metadata(
    metadata: &std::collections::HashMap<String, String>,
) -> Result<Option<agent::FSGroup>> {
    for (key, value) in metadata {
        match key.as_str() {
            "createFilesystem" if value == "false" => (),
            "fsGroup" | "fsGroupChangePolicy" => (),
            _ => anyhow::bail!("kata-fc-minimal: unsupported direct-volume metadata {key}={value}"),
        }
    }
    let policy = match metadata.get("fsGroupChangePolicy").map(String::as_str) {
        None | Some("Always") => agent::FSGroupChangePolicy::Always,
        Some("OnRootMismatch") => agent::FSGroupChangePolicy::OnRootMismatch,
        Some(other) => anyhow::bail!("kata-fc-minimal: unsupported fsGroupChangePolicy {other}"),
    };
    metadata
        .get("fsGroup")
        .map(|value| {
            Ok(agent::FSGroup {
                group_id: value
                    .parse::<u32>()
                    .context("invalid direct-volume fsGroup")?,
                group_change_policy: policy,
            })
        })
        .transpose()
}

#[cfg(test)]
mod metadata_tests {
    use super::*;
    #[test]
    fn direct_volume_preserves_workspace_group() {
        let group = supported_metadata(
            &[
                ("createFilesystem".into(), "false".into()),
                ("fsGroup".into(), "1000".into()),
                ("fsGroupChangePolicy".into(), "OnRootMismatch".into()),
            ]
            .into(),
        )
        .unwrap()
        .unwrap();
        assert_eq!(group.group_id, 1000);
        assert_eq!(
            group.group_change_policy,
            agent::FSGroupChangePolicy::OnRootMismatch
        );
        for pair in [
            ("createFilesystem", "true"),
            ("fsGroup", "-1"),
            ("fsGroupChangePolicy", "unknown"),
            ("unknown", "true"),
        ] {
            assert!(supported_metadata(&[(pair.0.into(), pair.1.into())].into()).is_err());
        }
        assert!(supported_metadata(&Default::default()).unwrap().is_none());
    }
}

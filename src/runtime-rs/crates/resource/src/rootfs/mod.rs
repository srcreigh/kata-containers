// Copyright (c) 2019-2022 Alibaba Cloud
// Copyright (c) 2019-2022 Ant Group
//
// SPDX-License-Identifier: Apache-2.0
//

use agent::Storage;
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use kata_types::mount::Mount;
mod block_rootfs;

use hypervisor::{device::device_manager::DeviceManager, Hypervisor};

use std::{collections::HashMap, sync::Arc, vec::Vec};
use tokio::sync::RwLock;

use self::block_rootfs::is_block_rootfs;
use oci_spec::runtime as oci;

const ROOTFS: &str = "rootfs";

#[async_trait]
pub trait Rootfs: Send + Sync {
    async fn get_guest_rootfs_path(&self) -> Result<String>;
    async fn get_rootfs_mount(&self) -> Result<Vec<oci::Mount>>;
    async fn get_storage(&self) -> Option<Vec<Storage>>;
    async fn cleanup(&self, device_manager: &RwLock<DeviceManager>) -> Result<()>;
    async fn get_device_id(&self) -> Result<Option<String>>;
}

#[derive(Default)]
struct RootFsResourceInner {
    rootfs: Vec<Arc<dyn Rootfs>>,
}

pub struct RootFsResource {
    inner: Arc<RwLock<RootFsResourceInner>>,
}

impl Default for RootFsResource {
    fn default() -> Self {
        Self::new()
    }
}

impl RootFsResource {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(RootFsResourceInner::default())),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn handler_rootfs(
        &self,
        device_manager: &RwLock<DeviceManager>,
        _h: &dyn Hypervisor,
        sid: &str,
        cid: &str,
        _root: &oci::Root,
        _bundle_path: &str,
        rootfs_mounts: &[Mount],
        _annotations: &HashMap<String, String>,
    ) -> Result<Arc<dyn Rootfs>> {
        anyhow::ensure!(
            rootfs_mounts.len() == 1,
            "kata-fc-minimal: exactly one block-backed rootfs is required (use devmapper)"
        );
        let (dev_id, layer) = is_block_rootfs(&rootfs_mounts[0]).ok_or_else(||
            anyhow!("kata-fc-minimal: shared, overlay, guest-pulled and multi-layer rootfs are unsupported; use devmapper"))?;
        let rootfs: Arc<dyn Rootfs> = Arc::new(
            block_rootfs::BlockRootfs::new(device_manager, sid, cid, dev_id, &layer)
                .await
                .context("new block rootfs")?,
        );
        self.inner.write().await.rootfs.push(rootfs.clone());
        Ok(rootfs)
    }

    pub async fn dump(&self) {
        let inner = self.inner.read().await;
        for r in &inner.rootfs {
            info!(
                sl!(),
                "rootfs {:?}: count {}",
                r.get_guest_rootfs_path().await,
                Arc::strong_count(r)
            );
        }
    }
}

// Copyright (c) 2019 Ant Financial
// Copyright (c) 2023 Alibaba Cloud
//
// SPDX-License-Identifier: Apache-2.0
//

use std::path::Path;
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use kata_types::mount::KATA_BLOCK_VOLUME_CREATE_FS;
use protocols::agent::Storage;

use crate::sandbox::Sandbox;
use crate::storage::mmio::get_virtio_blk_mmio_device_name;
use crate::storage::{common_storage_handler, new_device, StorageDevice};
use slog::Logger;
use tokio::sync::Mutex;

const MKFS_EXT4: &str = "mkfs.ext4";
const BLOCK_EMPTYDIR_EXT4_MKFS_OPTS: [&str; 8] =
    ["-O", "^has_journal", "-m", "0", "-i", "163840", "-I", "128"];

async fn handle_block_storage(logger: &Logger, storage: &Storage) -> Result<Arc<StorageDevice>> {
    if should_create_block_filesystem(storage) {
        ensure_block_filesystem(logger, storage).await?;
    }
    new_device(common_storage_handler(logger, storage)?)
}

fn should_create_block_filesystem(storage: &Storage) -> bool {
    storage
        .driver_options
        .iter()
        .any(|opt| opt == KATA_BLOCK_VOLUME_CREATE_FS)
}

async fn ensure_block_filesystem(logger: &Logger, storage: &Storage) -> Result<()> {
    match storage.fstype.as_str() {
        "ext4" => ensure_ext4_filesystem(logger, &storage.source).await,
        _ => Err(anyhow!(
            "creating filesystem {} for block storage is unsupported",
            storage.fstype
        )),
    }
}

async fn ensure_ext4_filesystem(logger: &Logger, source: &str) -> Result<()> {
    // This option is emitted for block emptyDir volumes, whose backing device
    // is ephemeral and freshly allocated for the pod.
    info!(logger, "creating ext4 filesystem"; "source" => source);
    let output = {
        // Keep the agent SIGCHLD handler from reaping this child before
        // tokio::process observes it.
        let _locker = rustjail::container::WAIT_PID_LOCKER.lock().await;
        tokio::process::Command::new(MKFS_EXT4)
            .arg("-F")
            .args(BLOCK_EMPTYDIR_EXT4_MKFS_OPTS)
            .arg(source)
            .output()
            .await
            .with_context(|| format!("run {MKFS_EXT4} for {source}"))?
    };

    if output.status.success() {
        return Ok(());
    }

    Err(anyhow!(
        "{} failed for {}: status={}, stdout={}, stderr={}",
        MKFS_EXT4,
        source,
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    ))
}

#[tracing::instrument(skip_all)]
pub(super) async fn create_device(
    storage: &Storage,
    logger: &Logger,
    sandbox: &Arc<Mutex<Sandbox>>,
) -> Result<Arc<StorageDevice>> {
    if !Path::new(&storage.source).exists() {
        get_virtio_blk_mmio_device_name(sandbox, &storage.source)
            .await
            .context("failed to get mmio device name")?;
    }
    handle_block_storage(logger, storage).await
}

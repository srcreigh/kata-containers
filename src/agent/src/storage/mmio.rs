// Copyright (c) 2019 Ant Financial
// Copyright (c) 2024 Intel Corporation
//
// SPDX-License-Identifier: Apache-2.0
//

use crate::sandbox::Sandbox;
use crate::uevent::{wait_for_uevent, Uevent, UeventMatcher};
use anyhow::{anyhow, Context, Result};
use std::sync::Arc;
use tokio::sync::Mutex;

pub async fn get_virtio_blk_mmio_device_name(
    sandbox: &Arc<Mutex<Sandbox>>,
    devpath: &str,
) -> Result<()> {
    let devname = devpath
        .strip_prefix("/dev/")
        .ok_or_else(|| anyhow!("Storage source '{}' must start with /dev/", devpath))?;

    let matcher = VirtioBlkMmioMatcher::new(devname);
    let uev = wait_for_uevent(sandbox, matcher)
        .await
        .context("failed to wait for uevent")?;
    if uev.devname != devname {
        return Err(anyhow!(
            "Unexpected device name {} for mmio device (expected {})",
            uev.devname,
            devname
        ));
    }
    Ok(())
}

#[derive(Debug)]
pub struct VirtioBlkMmioMatcher {
    suffix: String,
}

impl VirtioBlkMmioMatcher {
    pub fn new(devname: &str) -> VirtioBlkMmioMatcher {
        VirtioBlkMmioMatcher {
            suffix: format!(r"/block/{devname}"),
        }
    }
}

impl UeventMatcher for VirtioBlkMmioMatcher {
    fn is_match(&self, uev: &Uevent) -> bool {
        uev.subsystem == "block" && uev.devpath.ends_with(&self.suffix) && !uev.devname.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mmio_matcher_requires_exact_disk_and_block_subsystem() {
        let matcher = VirtioBlkMmioMatcher::new("vda");
        let mut event = Uevent::default();
        event.subsystem = "block".into();
        event.devpath = "/devices/platform/virtio-mmio/virtio0/block/vda".into();
        event.devname = "vda".into();
        assert!(matcher.is_match(&event));
        event.devpath.push_str("/vda1");
        assert!(!matcher.is_match(&event));
        event.devpath = "/devices/platform/virtio-mmio/virtio0/block/vdaa".into();
        assert!(!matcher.is_match(&event));
        event.devpath = "/devices/platform/virtio-mmio/virtio0/block/vda".into();
        event.subsystem = "net".into();
        assert!(!matcher.is_match(&event));
    }
}

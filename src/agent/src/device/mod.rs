// Copyright (c) 2019 Ant Financial
// Copyright (c) 2024 Intel Corporation
//
// SPDX-License-Identifier: Apache-2.0
//

use crate::sandbox::Sandbox;
use anyhow::{anyhow, Context, Result};
use kata_types::device::DRIVER_BLK_MMIO_TYPE;
use nix::sys::stat;
use oci::{LinuxDeviceCgroup, Spec};
use oci_spec::runtime as oci;
use protocols::agent::Device;
use slog::Logger;
use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::os::unix::prelude::FileTypeExt;
use std::sync::Arc;
use tokio::sync::Mutex;

pub const BLOCK: &str = "block";

#[derive(Debug, Clone)]
pub struct DeviceInfo {
    // The major and minor numbers for the device within the guest
    guest_major: i64,
    guest_minor: i64,
}

impl DeviceInfo {
    fn new(vm_path: &str) -> Result<Self> {
        let metadata = fs::metadata(vm_path).context("read MMIO device metadata")?;
        anyhow::ensure!(
            metadata.file_type().is_block_device(),
            "MMIO mapping requires a block device"
        );
        let devid = metadata.rdev();
        Ok(Self {
            guest_major: stat::major(devid) as i64,
            guest_minor: stat::minor(devid) as i64,
        })
    }
}

#[tracing::instrument(skip_all)]
pub async fn add_devices(
    logger: &Logger,
    devices: &[Device],
    spec: &mut Spec,
    sandbox: &Arc<Mutex<Sandbox>>,
) -> Result<()> {
    let mut dev_updates = HashMap::with_capacity(devices.len());
    for device in devices {
        anyhow::ensure!(
            device.type_ == DRIVER_BLK_MMIO_TYPE
                && device.options.is_empty()
                && !device.vm_path.is_empty()
                && !device.container_path.is_empty(),
            "kata-fc: only MMIO block devices with guest/container paths and no driver options are supported"
        );
        if !std::path::Path::new(&device.vm_path).exists() {
            crate::storage::mmio::get_virtio_blk_mmio_device_name(sandbox, &device.vm_path)
                .await
                .context("wait for MMIO device")?;
        }
        let info = DeviceInfo::new(&device.vm_path)?;
        if dev_updates
            .insert(device.container_path.as_str(), info.clone())
            .is_some()
        {
            return Err(anyhow!(
                "Conflicting device updates for {}",
                device.container_path
            ));
        }
        insert_devices_cgroup_rule(logger, spec, &info).context("Update device cgroup")?;
    }
    update_spec_devices(logger, spec, dev_updates)
}

// Insert a devices cgroup rule to control access to device.

#[tracing::instrument(skip_all)]
fn insert_devices_cgroup_rule(
    logger: &Logger,
    spec: &mut Spec,
    dev_info: &DeviceInfo,
) -> Result<()> {
    let linux = spec
        .linux_mut()
        .as_mut()
        .ok_or_else(|| anyhow!("Spec didn't container linux field"))?;
    let linux_resource = &mut oci::LinuxResources::default();
    let resource = linux.resources_mut().as_mut().unwrap_or(linux_resource);
    let mut device_cgrp = LinuxDeviceCgroup::default();
    device_cgrp.set_allow(true);
    device_cgrp.set_major(Some(dev_info.guest_major));
    device_cgrp.set_minor(Some(dev_info.guest_minor));
    device_cgrp.set_typ(Some(oci::LinuxDeviceType::B));
    device_cgrp.set_access(Some("rwm".into()));

    debug!(
        logger,
        "Insert a devices cgroup rule";
        "linux_device_cgroup" => device_cgrp.allow(),
        "guest_major" => device_cgrp.major(),
        "guest_minor" => device_cgrp.minor(),
        "type" => device_cgrp.typ().unwrap().as_str(),
        "access" => device_cgrp.access().as_ref().unwrap().as_str(),
    );

    if let Some(devices) = resource.devices_mut() {
        devices.push(device_cgrp);
    } else {
        resource.set_devices(Some(vec![device_cgrp]));
    }

    Ok(())
}

// update_spec_devices updates the device list in the OCI spec to make
// it include details appropriate for the VM, instead of the host.  It
// is given a map of (container_path => update) where:
//     container_path: the path to the device in the original OCI spec
//     update: information on changes to make to the device

#[tracing::instrument(skip_all)]
fn update_spec_devices(
    logger: &Logger,
    spec: &mut Spec,
    mut updates: HashMap<&str, DeviceInfo>,
) -> Result<()> {
    let linux = spec
        .linux_mut()
        .as_mut()
        .ok_or_else(|| anyhow!("Spec didn't contain linux field"))?;
    let mut res_updates = HashMap::<(String, i64, i64), DeviceInfo>::with_capacity(updates.len());

    let mut default_devices = Vec::new();
    let linux_devices = linux.devices_mut().as_mut().unwrap_or(&mut default_devices);
    for specdev in linux_devices.iter_mut() {
        let devtype = specdev.typ().as_str().to_string();
        if let Some(update) = updates.remove(specdev.path().clone().display().to_string().as_str())
        {
            let host_major = specdev.major();
            let host_minor = specdev.minor();

            info!(
                logger,
                "update_spec_devices() updating device";
                "container_path" => &specdev.path().display().to_string(),
                "type" => &devtype,
                "host_major" => host_major,
                "host_minor" => host_minor,
                "guest_major" => update.guest_major,
                "guest_minor" => update.guest_minor,
            );

            specdev.set_major(update.guest_major);
            specdev.set_minor(update.guest_minor);

            if res_updates
                .insert((devtype, host_major, host_minor), update)
                .is_some()
            {
                return Err(anyhow!(
                    "Conflicting resource updates for host_major={} host_minor={}",
                    host_major,
                    host_minor
                ));
            }
        }
    }

    // Make sure we applied all of our updates
    if !updates.is_empty() {
        return Err(anyhow!(
            "Missing devices in OCI spec: {:?}",
            updates
                .keys()
                .map(|d| format!("{d:?}"))
                .collect::<Vec<_>>()
                .join(" ")
        ));
    }

    if let Some(resources) = linux.resources_mut().as_mut() {
        if let Some(resources_devices) = resources.devices_mut().as_mut() {
            for d in resources_devices.iter_mut() {
                let dev_type = d.typ().unwrap_or_default().as_str().to_string();
                if let (Some(host_major), Some(host_minor)) = (d.major(), d.minor()) {
                    if let Some(update) =
                        res_updates.get(&(dev_type.clone(), host_major, host_minor))
                    {
                        info!(
                            logger,
                            "update_spec_devices() updating resource";
                            "type" => &dev_type,
                            "host_major" => host_major,
                            "host_minor" => host_minor,
                            "guest_major" => update.guest_major,
                            "guest_minor" => update.guest_minor,
                        );

                        d.set_major(Some(update.guest_major));
                        d.set_minor(Some(update.guest_minor));
                    }
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use oci::{
        LinuxBuilder, LinuxDeviceBuilder, LinuxDeviceCgroupBuilder, LinuxDeviceType,
        LinuxResourcesBuilder, SpecBuilder,
    };
    use std::path::PathBuf;

    fn logger() -> Logger {
        Logger::root(slog::Discard, o!())
    }

    fn info(major: i64, minor: i64) -> DeviceInfo {
        DeviceInfo {
            guest_major: major,
            guest_minor: minor,
        }
    }

    fn spec(devices: &[(&str, LinuxDeviceType, i64, i64)]) -> Spec {
        SpecBuilder::default()
            .linux(
                LinuxBuilder::default()
                    .devices(
                        devices
                            .iter()
                            .map(|(path, typ, major, minor)| {
                                LinuxDeviceBuilder::default()
                                    .path(PathBuf::from(path))
                                    .typ(*typ)
                                    .major(*major)
                                    .minor(*minor)
                                    .build()
                                    .unwrap()
                            })
                            .collect::<Vec<_>>(),
                    )
                    .resources(
                        LinuxResourcesBuilder::default()
                            .devices(
                                devices
                                    .iter()
                                    .map(|(_, typ, major, minor)| {
                                        LinuxDeviceCgroupBuilder::default()
                                            .allow(true)
                                            .typ(*typ)
                                            .major(*major)
                                            .minor(*minor)
                                            .access("rw")
                                            .build()
                                            .unwrap()
                                    })
                                    .collect::<Vec<_>>(),
                            )
                            .build()
                            .unwrap(),
                    )
                    .build()
                    .unwrap(),
            )
            .build()
            .unwrap()
    }

    #[test]
    fn mmio_metadata_rejects_character_devices_and_regular_files() {
        let file = tempfile::NamedTempFile::new().unwrap();
        for path in [
            "/dev/null",
            "/",
            "",
            "/nonexistent/device",
            file.path().to_str().unwrap(),
        ] {
            assert!(DeviceInfo::new(path).is_err(), "{}", path);
        }
    }

    #[test]
    fn block_mapping_preserves_paths_and_character_rules() {
        let mut spec = spec(&[
            ("/dev/block", LinuxDeviceType::B, 8, 0),
            ("/dev/char", LinuxDeviceType::C, 8, 0),
        ]);
        update_spec_devices(&logger(), &mut spec, [("/dev/block", info(252, 3))].into()).unwrap();
        let linux = spec.linux().as_ref().unwrap();
        let devices = linux.devices().as_ref().unwrap();
        assert_eq!(devices[0].path(), &PathBuf::from("/dev/block"));
        assert_eq!((devices[0].major(), devices[0].minor()), (252, 3));
        assert_eq!((devices[1].major(), devices[1].minor()), (8, 0));
        let rules = linux
            .resources()
            .as_ref()
            .unwrap()
            .devices()
            .as_ref()
            .unwrap();
        assert_eq!((rules[0].major(), rules[0].minor()), (Some(252), Some(3)));
        assert_eq!((rules[1].major(), rules[1].minor()), (Some(8), Some(0)));
    }

    #[test]
    fn overlapping_host_guest_numbers_are_remapped_once() {
        let mut spec = spec(&[
            ("/dev/a", LinuxDeviceType::B, 8, 0),
            ("/dev/b", LinuxDeviceType::B, 8, 1),
        ]);
        update_spec_devices(
            &logger(),
            &mut spec,
            [("/dev/a", info(8, 1)), ("/dev/b", info(8, 2))].into(),
        )
        .unwrap();
        let rules = spec
            .linux()
            .as_ref()
            .unwrap()
            .resources()
            .as_ref()
            .unwrap()
            .devices()
            .as_ref()
            .unwrap();
        assert_eq!(rules[0].minor(), Some(1));
        assert_eq!(rules[1].minor(), Some(2));
    }

    #[test]
    fn missing_and_conflicting_spec_mappings_are_errors() {
        assert!(update_spec_devices(
            &logger(),
            &mut Spec::default(),
            [("/dev/a", info(252, 1))].into()
        )
        .is_err());
        let mut missing = spec(&[]);
        assert!(
            update_spec_devices(&logger(), &mut missing, [("/dev/a", info(252, 1))].into())
                .is_err()
        );
        let mut duplicate = spec(&[
            ("/dev/a", LinuxDeviceType::B, 8, 0),
            ("/dev/b", LinuxDeviceType::B, 8, 0),
        ]);
        assert!(update_spec_devices(
            &logger(),
            &mut duplicate,
            [("/dev/a", info(252, 1)), ("/dev/b", info(252, 2))].into()
        )
        .is_err());
    }

    #[test]
    fn added_rule_is_read_write_mmio_block() {
        let mut spec = spec(&[]);
        insert_devices_cgroup_rule(&logger(), &mut spec, &info(252, 3)).unwrap();
        let rules = spec
            .linux()
            .as_ref()
            .unwrap()
            .resources()
            .as_ref()
            .unwrap()
            .devices()
            .as_ref()
            .unwrap();
        assert_eq!(rules.len(), 1);
        assert!(rules[0].allow());
        assert_eq!(rules[0].typ(), Some(LinuxDeviceType::B));
        assert_eq!(rules[0].access().as_deref(), Some("rwm"));
        assert_eq!((rules[0].major(), rules[0].minor()), (Some(252), Some(3)));
    }
}

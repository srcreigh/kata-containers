// Copyright (c) 2019 Ant Financial
// Copyright (c) 2024 Intel Corporation
//
// SPDX-License-Identifier: Apache-2.0
//

use self::block_device_handler::VirtioBlkMmioDeviceHandler;
use crate::sandbox::Sandbox;
use anyhow::{anyhow, Context, Result};
use kata_types::device::DeviceHandlerManager;
use nix::sys::stat;
use oci::{LinuxDeviceCgroup, Spec};
use oci_spec::runtime as oci;
use protocols::agent::Device;
use slog::Logger;
use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::os::unix::prelude::FileTypeExt;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;

pub mod block_device_handler;

pub const BLOCK: &str = "block";

#[derive(Debug, Clone)]
pub struct DeviceInfo {
    // Device type, "b" for block device and "c" for character device
    cgroup_type: String,
    // The major and minor numbers for the device within the guest
    guest_major: i64,
    guest_minor: i64,
}

impl DeviceInfo {
    /// Create a device info.
    ///
    /// # Arguments
    ///
    /// * `vm_path` - Device's vm path.
    /// * `is_rdev` - If the vm_path is a device, set to true. If the
    ///   vm_path is a file in a device, set to false.
    pub fn new(vm_path: &str, is_rdev: bool) -> Result<Self> {
        let cgroup_type;
        let devid;

        let vm_path = PathBuf::from(vm_path);
        if !vm_path.exists() {
            return Err(anyhow!("VM device path {:?} doesn't exist", vm_path));
        }

        let metadata = fs::metadata(&vm_path)?;

        if is_rdev {
            devid = metadata.rdev();
            let file_type = metadata.file_type();
            if file_type.is_block_device() {
                cgroup_type = String::from("b");
            } else if file_type.is_char_device() {
                cgroup_type = String::from("c");
            } else {
                return Err(anyhow!("Unknown device {:?}'s cgroup type", vm_path));
            }
        } else {
            devid = metadata.dev();
            cgroup_type = String::from("b");
        }

        let guest_major = stat::major(devid) as i64;
        let guest_minor = stat::minor(devid) as i64;

        Ok(DeviceInfo {
            cgroup_type,
            guest_major,
            guest_minor,
        })
    }
}

// Represents the device-node and resource related updates to the OCI
// spec needed for a particular device
#[derive(Debug, Clone)]
struct DevUpdate {
    info: DeviceInfo,
    // an optional new path to update the device to in the "inner" container
    // specification
    final_path: Option<String>,
}

impl DevUpdate {
    fn new(vm_path: &str, final_path: &str) -> Result<Self> {
        Ok(DevUpdate {
            final_path: Some(final_path.to_owned()),
            ..DeviceInfo::new(vm_path, true)?.into()
        })
    }
}

impl From<DeviceInfo> for DevUpdate {
    fn from(info: DeviceInfo) -> Self {
        DevUpdate {
            info,
            final_path: None,
        }
    }
}

// Represents the updates to the OCI spec needed for a particular device
#[derive(Debug, Clone, Default)]
pub struct SpecUpdate {
    dev: Option<DevUpdate>,
}

impl<T: Into<DevUpdate>> From<T> for SpecUpdate {
    fn from(dev: T) -> Self {
        SpecUpdate {
            dev: Some(dev.into()),
        }
    }
}

#[derive(Debug)]
pub struct DeviceContext<'a> {
    logger: &'a Logger,
    sandbox: &'a Arc<Mutex<Sandbox>>,
}

/// Trait object to handle device.
#[async_trait::async_trait]
pub trait DeviceHandler: Send + Sync {
    /// Handle the device
    async fn device_handler(&self, device: &Device, ctx: &mut DeviceContext) -> Result<SpecUpdate>;

    /// Return the driver types that the handler manages.
    fn driver_types(&self) -> &[&str];
}

#[rustfmt::skip]
lazy_static! {
    pub static ref DEVICE_HANDLERS: DeviceHandlerManager<Arc<dyn DeviceHandler>> = {
        let mut manager: DeviceHandlerManager<Arc<dyn DeviceHandler>> = DeviceHandlerManager::new();

        let handlers: Vec<Arc<dyn DeviceHandler>> = vec![
            Arc::new(VirtioBlkMmioDeviceHandler {}),
        ];

        for handler in handlers {
            manager.add_handler(handler.driver_types(), handler.clone()).unwrap();
        }
        manager
    };
}

#[tracing::instrument(skip_all)]
pub async fn add_devices(
    logger: &Logger,
    devices: &[Device],
    spec: &mut Spec,
    sandbox: &Arc<Mutex<Sandbox>>,
) -> Result<()> {
    let mut dev_updates = HashMap::<&str, DevUpdate>::with_capacity(devices.len());

    for device in devices.iter() {
        validate_device(logger, device)?;
        if let Some(handler) = DEVICE_HANDLERS.handler(&device.type_) {
            let mut ctx = DeviceContext { logger, sandbox };

            match handler.device_handler(device, &mut ctx).await {
                Ok(update) => {
                    if let Some(dev_update) = update.dev {
                        if dev_updates
                            .insert(&device.container_path, dev_update.clone())
                            .is_some()
                        {
                            return Err(anyhow!(
                                "Conflicting device updates for {}",
                                &device.container_path
                            ));
                        }

                        // Update cgroup to allow all devices added to guest.
                        insert_devices_cgroup_rule(logger, spec, &dev_update.info, true, "rwm")
                            .context("Update device cgroup")?;
                    }
                }
                Err(e) => {
                    error!(logger, "failed to add devices, error: {e:?}");
                    return Err(e);
                }
            }
        } else {
            return Err(anyhow!(
                "Failed to find the device handler {}",
                device.type_
            ));
        }
    }

    update_spec_devices(logger, spec, dev_updates)
}

#[tracing::instrument(skip_all)]
fn validate_device(logger: &Logger, device: &Device) -> Result<()> {
    // log before validation to help with debugging gRPC protocol version differences.
    info!(
        logger,
        "device-id: {}, device-type: {}, device-vm-path: {}, device-container-path: {}, device-options: {:?}",
        device.id, device.type_, device.vm_path, device.container_path, device.options
    );

    if device.type_.is_empty() {
        return Err(anyhow!("invalid type for device {:?}", device));
    }

    if device.id.is_empty() && device.vm_path.is_empty() {
        return Err(anyhow!("invalid ID and VM path for device {:?}", device));
    }

    if device.container_path.is_empty() {
        return Err(anyhow!("invalid container path for device {:?}", device));
    }
    Ok(())
}

// Insert a devices cgroup rule to control access to device.

#[tracing::instrument(skip_all)]
pub fn insert_devices_cgroup_rule(
    logger: &Logger,
    spec: &mut Spec,
    dev_info: &DeviceInfo,
    allow: bool,
    access: &str,
) -> Result<()> {
    let linux = spec
        .linux_mut()
        .as_mut()
        .ok_or_else(|| anyhow!("Spec didn't container linux field"))?;
    let devcgrp_type = dev_info
        .cgroup_type
        .parse::<oci::LinuxDeviceType>()
        .context(format!(
            "Failed to parse {:?} to Enum LinuxDeviceType",
            dev_info.cgroup_type
        ))?;
    let linux_resource = &mut oci::LinuxResources::default();
    let resource = linux.resources_mut().as_mut().unwrap_or(linux_resource);
    let mut device_cgrp = LinuxDeviceCgroup::default();
    device_cgrp.set_allow(allow);
    device_cgrp.set_major(Some(dev_info.guest_major));
    device_cgrp.set_minor(Some(dev_info.guest_minor));
    device_cgrp.set_typ(Some(devcgrp_type));
    device_cgrp.set_access(Some(access.to_owned()));

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
    mut updates: HashMap<&str, DevUpdate>,
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
                "guest_major" => update.info.guest_major,
                "guest_minor" => update.info.guest_minor,
                "final_path" => update.final_path.as_ref(),
            );

            specdev.set_major(update.info.guest_major);
            specdev.set_minor(update.info.guest_minor);
            if let Some(final_path) = update.final_path {
                specdev.set_path(PathBuf::from(&final_path));
            }

            if res_updates
                .insert((devtype, host_major, host_minor), update.info)
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
        Linux, LinuxBuilder, LinuxDeviceBuilder, LinuxDeviceCgroupBuilder, LinuxDeviceType,
        LinuxResources, LinuxResourcesBuilder, SpecBuilder,
    };
    use oci_spec::runtime as oci;
    use rstest::rstest;
    use std::iter::FromIterator;
    use tempfile::tempdir;

    const VM_ROOTFS: &str = "/";
    const TEST_CONTAINER_PATH: &str = "/dev/null";
    const TEST_VM_PATH: &str = "/dev/null";
    const TEST_MAJOR: i64 = 7;
    const TEST_MINOR: i64 = 2;

    // Helper function to create a test logger
    fn create_test_logger() -> slog::Logger {
        slog::Logger::root(slog::Discard, o!())
    }

    // Helper function to create a device update map
    fn create_device_update<'a>(
        container_path: &'a str,
        vm_path: &str,
    ) -> HashMap<&'a str, DevUpdate> {
        HashMap::from_iter(vec![(
            container_path,
            DevUpdate::new(container_path, vm_path).unwrap(),
        )])
    }

    #[test]
    fn test_dev_update_new() {
        let result = DevUpdate::new("/dev/null", "/dev/null");
        assert!(result.is_ok());

        let update = result.unwrap();
        assert_eq!(update.final_path, Some("/dev/null".to_string()));

        let result2 = DevUpdate::new("/dev/null", "/dev/custom");
        assert!(result2.is_ok());
        let update2 = result2.unwrap();
        assert_eq!(update2.final_path, Some("/dev/custom".to_string()));

        let result_invalid = DevUpdate::new("/nonexistent/device", "/dev/null");
        assert!(result_invalid.is_err());
    }

    #[rstest]
    #[case::char_device("/dev/null", true, "c", true)]
    #[case::block_device("/", false, "b", true)]
    #[case::nonexistent("/nonexistent/path", true, "", false)]
    #[case::empty_path("", true, "", false)]
    #[test]
    fn test_device_info_new(
        #[case] path: &str,
        #[case] is_char: bool,
        #[case] expected_type: &str,
        #[case] should_succeed: bool,
    ) {
        let result = DeviceInfo::new(path, is_char);

        if should_succeed {
            let info = result.unwrap();
            assert_eq!(info.cgroup_type, expected_type);
            assert!(info.guest_major >= 0);
            assert!(info.guest_minor >= 0);
        } else {
            assert!(result.is_err());
        }
    }

    #[test]
    fn test_spec_update_conversions() {
        let info = DeviceInfo::new("/dev/null", true).unwrap();
        let spec_update: SpecUpdate = info.into();
        assert!(spec_update.dev.is_some());

        let dev_update = DevUpdate::new("/dev/null", "/dev/null").unwrap();
        let spec_update2: SpecUpdate = dev_update.into();
        assert!(spec_update2.dev.is_some());

        let spec_update3 = SpecUpdate::default();
        assert!(spec_update3.dev.is_none());
    }

    #[test]
    fn test_update_device_cgroup() {
        let logger = create_test_logger();
        let mut linux = Linux::default();
        linux.set_resources(Some(LinuxResources::default()));
        let mut spec = SpecBuilder::default().linux(linux).build().unwrap();

        let dev_info = DeviceInfo::new(VM_ROOTFS, false).unwrap();
        insert_devices_cgroup_rule(&logger, &mut spec, &dev_info, false, "rw").unwrap();

        let devices = spec
            .linux()
            .as_ref()
            .unwrap()
            .resources()
            .as_ref()
            .unwrap()
            .devices()
            .clone()
            .unwrap();
        assert_eq!(devices.len(), 1);

        let meta = fs::metadata(VM_ROOTFS).unwrap();
        let rdev = meta.dev();
        let major = stat::major(rdev) as i64;
        let minor = stat::minor(rdev) as i64;

        assert_eq!(devices[0].major(), Some(major));
        assert_eq!(devices[0].minor(), Some(minor));
    }

    #[test]
    fn test_update_spec_devices() {
        let logger = create_test_logger();
        let mut spec = Spec::default();

        // vm_path empty
        let update = DeviceInfo::new("", true);
        assert!(update.is_err());

        // linux is empty
        let res = update_spec_devices(
            &logger,
            &mut spec,
            create_device_update(TEST_CONTAINER_PATH, TEST_VM_PATH),
        );
        assert!(res.is_err());

        spec.set_linux(Some(Linux::default()));

        // linux.devices doesn't contain the updated device
        let res = update_spec_devices(
            &logger,
            &mut spec,
            create_device_update(TEST_CONTAINER_PATH, TEST_VM_PATH),
        );
        assert!(res.is_err());

        spec.linux_mut()
            .as_mut()
            .unwrap()
            .set_devices(Some(vec![LinuxDeviceBuilder::default()
                .path(PathBuf::from("/dev/null2"))
                .major(TEST_MAJOR)
                .minor(TEST_MINOR)
                .build()
                .unwrap()]));

        // guest and host path are not the same
        let res = update_spec_devices(
            &logger,
            &mut spec,
            create_device_update(TEST_CONTAINER_PATH, TEST_VM_PATH),
        );
        assert!(
            res.is_err(),
            "container_path={:?} vm_path={:?} spec={:?}",
            TEST_CONTAINER_PATH,
            TEST_VM_PATH,
            spec
        );

        spec.linux_mut()
            .as_mut()
            .unwrap()
            .devices_mut()
            .as_mut()
            .unwrap()[0]
            .set_path(PathBuf::from(TEST_CONTAINER_PATH));

        // spec.linux.resources is empty
        let res = update_spec_devices(
            &logger,
            &mut spec,
            create_device_update(TEST_CONTAINER_PATH, TEST_VM_PATH),
        );
        assert!(res.is_ok());

        // update both devices and cgroup lists
        spec.linux_mut()
            .as_mut()
            .unwrap()
            .set_devices(Some(vec![LinuxDeviceBuilder::default()
                .path(PathBuf::from(TEST_CONTAINER_PATH))
                .major(TEST_MAJOR)
                .minor(TEST_MINOR)
                .build()
                .unwrap()]));

        spec.linux_mut().as_mut().unwrap().set_resources(Some(
            oci::LinuxResourcesBuilder::default()
                .devices(vec![LinuxDeviceCgroupBuilder::default()
                    .major(TEST_MAJOR)
                    .minor(TEST_MINOR)
                    .build()
                    .unwrap()])
                .build()
                .unwrap(),
        ));

        let res = update_spec_devices(
            &logger,
            &mut spec,
            create_device_update(TEST_CONTAINER_PATH, TEST_VM_PATH),
        );
        assert!(res.is_ok());
    }

    #[test]
    fn test_update_spec_devices_guest_host_conflict() {
        let logger = create_test_logger();

        let null_rdev = fs::metadata("/dev/null").unwrap().rdev();
        let zero_rdev = fs::metadata("/dev/zero").unwrap().rdev();
        let full_rdev = fs::metadata("/dev/full").unwrap().rdev();

        let host_major_a = stat::major(null_rdev) as i64;
        let host_minor_a = stat::minor(null_rdev) as i64;
        let host_major_b = stat::major(zero_rdev) as i64;
        let host_minor_b = stat::minor(zero_rdev) as i64;

        let mut spec = SpecBuilder::default()
            .linux(
                LinuxBuilder::default()
                    .devices(vec![
                        LinuxDeviceBuilder::default()
                            .path(PathBuf::from("/dev/a"))
                            .typ(LinuxDeviceType::C)
                            .major(host_major_a)
                            .minor(host_minor_a)
                            .build()
                            .unwrap(),
                        LinuxDeviceBuilder::default()
                            .path(PathBuf::from("/dev/b"))
                            .typ(LinuxDeviceType::C)
                            .major(host_major_b)
                            .minor(host_minor_b)
                            .build()
                            .unwrap(),
                    ])
                    .resources(
                        LinuxResourcesBuilder::default()
                            .devices(vec![
                                LinuxDeviceCgroupBuilder::default()
                                    .typ(LinuxDeviceType::C)
                                    .major(host_major_a)
                                    .minor(host_minor_a)
                                    .build()
                                    .unwrap(),
                                LinuxDeviceCgroupBuilder::default()
                                    .typ(LinuxDeviceType::C)
                                    .major(host_major_b)
                                    .minor(host_minor_b)
                                    .build()
                                    .unwrap(),
                            ])
                            .build()
                            .unwrap(),
                    )
                    .build()
                    .unwrap(),
            )
            .build()
            .unwrap();

        let container_path_a = "/dev/a";
        let vm_path_a = "/dev/zero";

        let guest_major_a = stat::major(zero_rdev) as i64;
        let guest_minor_a = stat::minor(zero_rdev) as i64;

        let container_path_b = "/dev/b";
        let vm_path_b = "/dev/full";

        let guest_major_b = stat::major(full_rdev) as i64;
        let guest_minor_b = stat::minor(full_rdev) as i64;

        let specdevices = &spec.linux().as_ref().unwrap().devices().clone().unwrap();
        assert_eq!(host_major_a, specdevices[0].major());
        assert_eq!(host_minor_a, specdevices[0].minor());
        assert_eq!(host_major_b, specdevices[1].major());
        assert_eq!(host_minor_b, specdevices[1].minor());

        let specresources_devices = spec
            .linux()
            .as_ref()
            .unwrap()
            .resources()
            .as_ref()
            .unwrap()
            .devices()
            .clone()
            .unwrap();
        assert_eq!(Some(host_major_a), specresources_devices[0].major());
        assert_eq!(Some(host_minor_a), specresources_devices[0].minor());
        assert_eq!(Some(host_major_b), specresources_devices[1].major());
        assert_eq!(Some(host_minor_b), specresources_devices[1].minor());

        let updates = HashMap::from_iter(vec![
            (
                container_path_a,
                DeviceInfo::new(vm_path_a, true).unwrap().into(),
            ),
            (
                container_path_b,
                DeviceInfo::new(vm_path_b, true).unwrap().into(),
            ),
        ]);
        let res = update_spec_devices(&logger, &mut spec, updates);
        assert!(res.is_ok());

        let specdevices = &spec.linux().as_ref().unwrap().devices().clone().unwrap();
        assert_eq!(guest_major_a, specdevices[0].major());
        assert_eq!(guest_minor_a, specdevices[0].minor());
        assert_eq!(guest_major_b, specdevices[1].major());
        assert_eq!(guest_minor_b, specdevices[1].minor());

        let specresources_devices = spec
            .linux()
            .as_ref()
            .unwrap()
            .resources()
            .as_ref()
            .unwrap()
            .devices()
            .clone()
            .unwrap();
        assert_eq!(Some(guest_major_a), specresources_devices[0].major());
        assert_eq!(Some(guest_minor_a), specresources_devices[0].minor());
        assert_eq!(Some(guest_major_b), specresources_devices[1].major());
        assert_eq!(Some(guest_minor_b), specresources_devices[1].minor());
    }

    #[test]
    fn test_update_spec_devices_char_block_conflict() {
        let logger = create_test_logger();

        let null_rdev = fs::metadata("/dev/null").unwrap().rdev();

        let guest_major = stat::major(null_rdev) as i64;
        let guest_minor = stat::minor(null_rdev) as i64;
        let host_major: i64 = 99;
        let host_minor: i64 = 99;

        let mut spec = SpecBuilder::default()
            .linux(
                LinuxBuilder::default()
                    .devices(vec![
                        LinuxDeviceBuilder::default()
                            .path(PathBuf::from("/dev/char"))
                            .typ(LinuxDeviceType::C)
                            .major(host_major)
                            .minor(host_minor)
                            .build()
                            .unwrap(),
                        LinuxDeviceBuilder::default()
                            .path(PathBuf::from("/dev/block"))
                            .typ(LinuxDeviceType::B)
                            .major(host_major)
                            .minor(host_minor)
                            .build()
                            .unwrap(),
                    ])
                    .resources(
                        LinuxResourcesBuilder::default()
                            .devices(vec![
                                LinuxDeviceCgroupBuilder::default()
                                    .typ(LinuxDeviceType::C)
                                    .major(host_major)
                                    .minor(host_minor)
                                    .build()
                                    .unwrap(),
                                LinuxDeviceCgroupBuilder::default()
                                    .typ(LinuxDeviceType::B)
                                    .major(host_major)
                                    .minor(host_minor)
                                    .build()
                                    .unwrap(),
                            ])
                            .build()
                            .unwrap(),
                    )
                    .build()
                    .unwrap(),
            )
            .build()
            .unwrap();

        let container_path = "/dev/char";
        let vm_path = "/dev/null";

        let specresources_devices = spec
            .linux()
            .as_ref()
            .unwrap()
            .resources()
            .as_ref()
            .unwrap()
            .devices()
            .clone()
            .unwrap();
        assert_eq!(Some(host_major), specresources_devices[0].major());
        assert_eq!(Some(host_minor), specresources_devices[0].minor());
        assert_eq!(Some(host_major), specresources_devices[1].major());
        assert_eq!(Some(host_minor), specresources_devices[1].minor());

        let res = update_spec_devices(
            &logger,
            &mut spec,
            HashMap::from_iter(vec![(
                container_path,
                DeviceInfo::new(vm_path, true).unwrap().into(),
            )]),
        );
        assert!(res.is_ok());

        // Only the char device, not the block device should be updated
        let specresources_devices = spec
            .linux()
            .as_ref()
            .unwrap()
            .resources()
            .as_ref()
            .unwrap()
            .devices()
            .clone()
            .unwrap();
        assert_eq!(Some(guest_major), specresources_devices[0].major());
        assert_eq!(Some(guest_minor), specresources_devices[0].minor());
        assert_eq!(Some(host_major), specresources_devices[1].major());
        assert_eq!(Some(host_minor), specresources_devices[1].minor());
    }

    #[test]
    fn test_update_spec_devices_final_path() {
        let logger = create_test_logger();

        let null_rdev = fs::metadata("/dev/null").unwrap().rdev();
        let guest_major = stat::major(null_rdev) as i64;
        let guest_minor = stat::minor(null_rdev) as i64;

        let container_path = "/dev/original";
        let host_major: i64 = 99;
        let host_minor: i64 = 99;

        let mut spec = SpecBuilder::default()
            .linux(
                LinuxBuilder::default()
                    .devices(vec![LinuxDeviceBuilder::default()
                        .path(PathBuf::from(container_path))
                        .typ(LinuxDeviceType::C)
                        .major(host_major)
                        .minor(host_minor)
                        .build()
                        .unwrap()])
                    .build()
                    .unwrap(),
            )
            .build()
            .unwrap();

        let vm_path = "/dev/null";
        let final_path = "/dev/new";

        let res = update_spec_devices(
            &logger,
            &mut spec,
            HashMap::from_iter(vec![(
                container_path,
                DevUpdate::new(vm_path, final_path).unwrap(),
            )]),
        );
        assert!(res.is_ok());

        let specdevices = &spec.linux().as_ref().unwrap().devices().clone().unwrap();
        assert_eq!(guest_major, specdevices[0].major());
        assert_eq!(guest_minor, specdevices[0].minor());
        assert_eq!(&PathBuf::from(final_path), specdevices[0].path());
    }
}

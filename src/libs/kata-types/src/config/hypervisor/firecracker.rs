// Copyright (c) 2019-2021 Alibaba Cloud
// Copyright (c) 2022-2023 Nubificus LTD
//
// SPDX-License-Identifier: Apache-2.0
//

use std::io::Result;
use std::path::Path;
use std::sync::Arc;

use super::{default, register_hypervisor_plugin};

use crate::config::default::MAX_FIRECRACKER_VCPUS;
use crate::config::default::MIN_FIRECRACKER_MEMORY_SIZE_MB;

use crate::config::{ConfigPlugin, TomlConfig};
use crate::validate_path;

/// Hypervisor name for firecracker, used to index `TomlConfig::hypervisor`.
pub const HYPERVISOR_NAME_FIRECRACKER: &str = "firecracker";

/// Configuration information for firecracker.
#[derive(Default, Debug)]
pub struct FirecrackerConfig {}

impl FirecrackerConfig {
    /// Create a new instance of `FirecrackerConfig`.
    pub fn new() -> Self {
        FirecrackerConfig {}
    }

    /// Register the firecracker plugin.
    pub fn register(self) {
        let plugin = Arc::new(self);
        register_hypervisor_plugin(HYPERVISOR_NAME_FIRECRACKER, plugin);
    }
}

fn reject_confidential_guest(hypervisor: &super::Hypervisor) -> Result<()> {
    if hypervisor.security_info.confidential_guest || !hypervisor.security_info.initdata.is_empty()
    {
        return Err(std::io::Error::other(
            "kata-fc: confidential guests and initdata are unsupported",
        ));
    }
    Ok(())
}

impl ConfigPlugin for FirecrackerConfig {
    fn get_max_cpus(&self) -> u32 {
        MAX_FIRECRACKER_VCPUS
    }

    fn get_min_memory(&self) -> u32 {
        MIN_FIRECRACKER_MEMORY_SIZE_MB
    }

    fn name(&self) -> &str {
        HYPERVISOR_NAME_FIRECRACKER
    }

    /// Adjust the configuration information after loading from configuration file.
    fn adjust_config(&self, conf: &mut TomlConfig) -> Result<()> {
        if let Some(firecracker) = conf.hypervisor.get_mut(HYPERVISOR_NAME_FIRECRACKER) {
            reject_confidential_guest(firecracker)?;
            if firecracker.boot_info.vm_rootfs_driver.is_empty() {
                firecracker.boot_info.vm_rootfs_driver = super::VIRTIO_BLK_MMIO.into();
            }
            if firecracker.blockdev_info.block_device_driver.is_empty() {
                firecracker.blockdev_info.block_device_driver = super::VIRTIO_BLK_MMIO.into();
            }
            if firecracker.boot_info.kernel.is_empty() {
                firecracker.boot_info.kernel =
                    default::DEFAULT_FIRECRACKER_GUEST_KERNEL_IMAGE.to_string();
            }
            if firecracker.boot_info.kernel_params.is_empty() {
                firecracker.boot_info.kernel_params =
                    default::DEFAULT_FIRECRACKER_GUEST_KERNEL_PARAMS.to_string();
            }
            if firecracker.machine_info.entropy_source.is_empty() {
                firecracker.machine_info.entropy_source =
                    default::DEFAULT_FIRECRACKER_ENTROPY_SOURCE.to_string();
            }

            if firecracker.memory_info.default_memory == 0 {
                firecracker.memory_info.default_memory =
                    default::DEFAULT_FIRECRACKER_MEMORY_SIZE_MB;
            }
        }

        Ok(())
    }

    /// Validate the configuration information.
    fn validate(&self, conf: &TomlConfig) -> Result<()> {
        if let Some(firecracker) = conf.hypervisor.get(HYPERVISOR_NAME_FIRECRACKER) {
            reject_confidential_guest(firecracker)?;
            if firecracker.path.is_empty() {
                return Err(std::io::Error::other("Firecracker path is empty"));
            }
            validate_path!(
                firecracker.path,
                "FIRECRACKER binary path `{}` is invalid: {}"
            )?;
            if firecracker.boot_info.kernel.is_empty() {
                return Err(std::io::Error::other(
                    "Guest kernel image for firecracker is empty",
                ));
            }
            if firecracker.boot_info.image.is_empty() {
                return Err(std::io::Error::other(
                    "Both guest boot image and initrd for firecracker are empty",
                ));
            }

            if (firecracker.cpu_info.default_vcpus > 0.0
                && firecracker.cpu_info.default_vcpus as u32 > default::MAX_FIRECRACKER_VCPUS)
                || firecracker.cpu_info.default_maxvcpus > default::MAX_FIRECRACKER_VCPUS
            {
                return Err(std::io::Error::other(format!(
                    "Firecracker hypervisor can not support {} vCPUs",
                    firecracker.cpu_info.default_maxvcpus,
                )));
            }

            if firecracker.memory_info.default_memory < MIN_FIRECRACKER_MEMORY_SIZE_MB {
                return Err(std::io::Error::other(format!(
                    "Firecracker hypervisor has minimal memory limitation {MIN_FIRECRACKER_MEMORY_SIZE_MB}",
                )));
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod minimal_tests {
    use super::*;
    #[test]
    fn boot_image_and_container_disks_default_to_mmio() {
        let mut conf = TomlConfig::default();
        conf.hypervisor
            .insert("firecracker".into(), Default::default());
        FirecrackerConfig::new().adjust_config(&mut conf).unwrap();
        let h = conf.hypervisor.get_mut("firecracker").unwrap();
        // Avoid filesystem lookups while exercising generic default adjustment.
        h.boot_info.kernel.clear();
        h.boot_info.adjust_config().unwrap();
        assert_eq!(h.boot_info.vm_rootfs_driver, super::super::VIRTIO_BLK_MMIO);
        assert_eq!(
            h.blockdev_info.block_device_driver,
            super::super::VIRTIO_BLK_MMIO
        );
    }

    #[test]
    fn firecracker_configuration_and_annotations_remain_usable() {
        FirecrackerConfig::new().register();
        let content = r#"
[hypervisor.firecracker]
path = "/dev/null"
jailer_path = "/dev/null"
kernel = "/dev/null"
image = "/dev/null"
rootfs_type = "ext4"
shared_fs = "none"
default_vcpus = 1
default_maxvcpus = 2
default_memory = 256
memory_slots = 1
enable_annotations = ["default_memory", "default_vcpus", "kernel"]
[agent.kata]
[runtime]
name = "virt_container"
hypervisor_name = "firecracker"
agent_name = "kata"
static_sandbox_resource_mgmt = true
sandbox_cgroup_only = true
disable_guest_seccomp = true
"#;
        let mut conf = TomlConfig::load(content).unwrap();
        conf.validate().unwrap();
        let annotations = crate::annotations::Annotation::new(std::collections::HashMap::from([
            (
                crate::annotations::KATA_ANNO_CFG_HYPERVISOR_DEFAULT_MEMORY.into(),
                "512MiB".into(),
            ),
            (
                crate::annotations::KATA_ANNO_CFG_HYPERVISOR_DEFAULT_VCPUS.into(),
                "2".into(),
            ),
            (
                crate::annotations::KATA_ANNO_CFG_HYPERVISOR_KERNEL_PATH.into(),
                "/dev/null".into(),
            ),
        ]));
        annotations.update_config_by_annotation(&mut conf).unwrap();
        conf.validate().unwrap();
        let fc = &conf.hypervisor["firecracker"];
        assert_eq!(fc.memory_info.default_memory, 512);
        assert_eq!(fc.cpu_info.default_vcpus, 2.0);
        assert_eq!(fc.boot_info.vm_rootfs_driver, "virtio-blk-mmio");
    }

    #[test]
    fn unsupported_backends_fail_before_path_operations() {
        for name in ["qemu", "clh", "dragonball", "openvmm", "remote", "unknown"] {
            for content in [
                format!("[hypervisor.{name}]\npath='/nonexistent/hypervisor'"),
                format!("[runtime]\nhypervisor_name='{name}'"),
            ] {
                let err = TomlConfig::load(&content).unwrap_err();
                assert!(err.to_string().contains("unsupported hypervisor"), "{err}");
            }
        }
    }

    #[test]
    fn confidential_config_rejected_without_decoding() {
        FirecrackerConfig::new().register();
        for option in [
            "confidential_guest = true",
            "initdata = 'not-base64-or-toml'",
        ] {
            let err = TomlConfig::load(&format!("[hypervisor.firecracker]\n{option}")).unwrap_err();
            assert!(err
                .to_string()
                .contains("confidential guests and initdata are unsupported"));
        }
        for value in ["", "invalid_base64!!", "H4sIAAAAAAAA"] {
            let annotations =
                crate::annotations::Annotation::new(std::collections::HashMap::from([(
                    crate::annotations::KATA_ANNO_CFG_HYPERVISOR_INIT_DATA.into(),
                    value.into(),
                )]));
            let err = annotations
                .update_config_by_annotation(&mut TomlConfig::default())
                .unwrap_err();
            assert!(err.to_string().contains("initdata is unsupported"));
        }
    }
}

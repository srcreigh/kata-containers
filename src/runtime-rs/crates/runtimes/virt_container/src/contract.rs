// SPDX-License-Identifier: Apache-2.0
//! The supported host contract. Reject excluded features before starting a VM.
use anyhow::{ensure, Result};
use kata_types::config::TomlConfig;

pub fn validate(c: &TomlConfig) -> Result<()> {
    let fail = "kata-fc-minimal: unsupported configuration";
    ensure!(
        c.runtime.hypervisor_name == "firecracker",
        "{fail}: only Firecracker is supported"
    );
    ensure!(
        c.runtime.name == "virt_container",
        "{fail}: only VM containers are supported"
    );
    ensure!(
        c.hypervisor.len() == 1,
        "{fail}: additional hypervisor configurations"
    );
    let h = c
        .hypervisor
        .get("firecracker")
        .ok_or_else(|| anyhow::anyhow!("{fail}: missing Firecracker configuration"))?;
    ensure!(
        !h.jailer_path.is_empty(),
        "{fail}: Firecracker jailer is required"
    );
    ensure!(
        !h.security_info.disable_seccomp,
        "{fail}: Firecracker seccomp cannot be disabled"
    );
    ensure!(
        !h.security_info.confidential_guest && h.security_info.initdata.is_empty(),
        "{fail}: confidential computing"
    );
    ensure!(!h.security_info.rootless, "{fail}: rootless host runtime");
    ensure!(
        h.security_info.guest_hook_path.is_empty(),
        "{fail}: guest hooks"
    );
    ensure!(
        !h.factory.enable_template
            && !h.vm_template.boot_from_template
            && !h.vm_template.boot_to_be_template,
        "{fail}: VM templates"
    );
    ensure!(
        h.shared_fs
            .shared_fs
            .as_deref()
            .is_none_or(|v| v.is_empty() || v == "none"),
        "{fail}: shared host filesystems"
    );
    ensure!(
        h.blockdev_info.block_device_driver == "virtio-blk-mmio"
            && h.boot_info.vm_rootfs_driver == "virtio-blk-mmio",
        "{fail}: only virtio-blk-mmio storage is supported"
    );
    ensure!(
        h.boot_info.initrd.is_empty(),
        "{fail}: Firecracker initrd boot"
    );
    ensure!(!h.memory_info.enable_guest_swap, "{fail}: guest swap");
    let b = &h.blockdev_info;
    ensure!(
        matches!(b.block_device_aio.as_str(), "" | "io_uring")
            && !b.block_device_cache_set
            && !b.block_device_cache_direct
            && !b.block_device_cache_noflush
            && b.block_device_logical_sector_size == 0
            && b.block_device_physical_sector_size == 0
            && matches!(b.num_queues, 0 | 1)
            && matches!(b.queue_size, 0 | 128)
            && b.memory_offset == 0,
        "{fail}: alternate block I/O, cache, sector or queue settings"
    );
    ensure!(
        !h.blockdev_info.enable_vhost_user_store,
        "{fail}: vhost-user storage"
    );
    ensure!(
        !h.device_info.enable_iommu && !h.device_info.enable_iommu_platform,
        "{fail}: IOMMU/device passthrough"
    );
    ensure!(
        matches!(h.device_info.cold_plug_vfio.as_str(), "" | "no-port")
            && h.device_info.pcie_root_port == 0
            && h.device_info.pcie_switch_port == 0,
        "{fail}: VFIO cold-plug/PCIe ports"
    );
    ensure!(
        h.guest_extension_images.is_empty(),
        "{fail}: guest extension images"
    );
    ensure!(
        c.runtime.static_sandbox_resource_mgmt,
        "{fail}: dynamic VM sizing"
    );
    ensure!(
        c.runtime.internetworking_model == "tcfilter",
        "{fail}: only tcfilter networking is supported"
    );
    ensure!(!c.runtime.disable_new_netns, "{fail}: host networking");
    ensure!(!c.runtime.enable_vcpus_pinning, "{fail}: vCPU pinning");
    ensure!(
        c.runtime.sandbox_cgroup_only,
        "{fail}: per-container host cgroups"
    );
    ensure!(
        c.runtime.experimental.is_empty(),
        "{fail}: experimental features"
    );
    ensure!(
        c.runtime.sandbox_bind_mounts.is_empty() && c.runtime.shared_mounts.is_empty(),
        "{fail}: sandbox/shared host mounts"
    );
    ensure!(
        c.runtime.emptydir_mode != "block-encrypted",
        "{fail}: encrypted emptyDir"
    );
    ensure!(!c.runtime.use_passfd_io, "{fail}: passfd IO");
    ensure!(
        !c.runtime.enable_pprof,
        "{fail}: profiling is not implemented in the Rust shim"
    );
    let a = c
        .agent
        .get("kata")
        .ok_or_else(|| anyhow::anyhow!("{fail}: missing kata agent"))?;
    ensure!(a.policy.is_empty(), "{fail}: agent policy");
    ensure!(!a.debug_console_enabled, "{fail}: guest debug console");
    ensure!(!a.visible_cdi_devices, "{fail}: agent CDI");
    ensure!(
        a.kernel_modules.is_empty(),
        "{fail}: runtime kernel-module loading"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn supported() -> TomlConfig {
        let mut c = TomlConfig::default();
        c.runtime.hypervisor_name = "firecracker".into();
        c.runtime.name = "virt_container".into();
        c.runtime.internetworking_model = "tcfilter".into();
        c.runtime.static_sandbox_resource_mgmt = true;
        c.runtime.sandbox_cgroup_only = true;
        let mut h = kata_types::config::Hypervisor::default();
        h.jailer_path = "/opt/kata/bin/jailer".into();
        h.blockdev_info.block_device_driver = "virtio-blk-mmio".into();
        h.boot_info.vm_rootfs_driver = "virtio-blk-mmio".into();
        c.hypervisor.insert("firecracker".into(), h);
        c.agent.insert("kata".into(), Default::default());
        c
    }
    #[test]
    fn supported_contract() {
        validate(&supported()).unwrap();
    }
    #[test]
    fn tracing_annotations_enable_both_sides() {
        kata_types::config::FirecrackerConfig::new().register();
        let mut config = supported();
        config.runtime.agent_name = "kata".into();
        let file = tempfile::NamedTempFile::new().unwrap();
        let path = file.path().to_str().unwrap().to_owned();
        let hv = config.hypervisor.get_mut("firecracker").unwrap();
        hv.path = path.clone();
        hv.jailer_path = path.clone();
        hv.boot_info.kernel = path.clone();
        hv.boot_info.image = path;
        config.agent.insert("kata".into(), Default::default());
        let annotations = [
            (
                "io.katacontainers.config.runtime.enable_tracing".into(),
                "true".into(),
            ),
            (
                "io.katacontainers.config.agent.enable_tracing".into(),
                "true".into(),
            ),
        ]
        .into();
        validate_annotations(&annotations).unwrap();
        kata_types::annotations::Annotation::new(annotations)
            .update_config_by_annotation(&mut config)
            .unwrap();
        validate(&config).unwrap();
        assert!(config.runtime.enable_tracing);
        assert!(config.agent["kata"].enable_tracing);
        assert_eq!(
            config.get_agent_kernel_params().unwrap()["agent.trace"],
            "true"
        );
    }

    #[test]
    fn excluded_features_fail_closed() {
        let mutations: Vec<fn(&mut TomlConfig)> = vec![
            |c| c.runtime.hypervisor_name = "qemu".into(),
            |c| c.runtime.name = "linux_container".into(),
            |c| {
                c.hypervisor
                    .get_mut("firecracker")
                    .unwrap()
                    .boot_info
                    .vm_rootfs_driver = "virtio-blk-pci".into()
            },
            |c| {
                c.hypervisor
                    .get_mut("firecracker")
                    .unwrap()
                    .memory_info
                    .enable_guest_swap = true
            },
            |c| {
                c.hypervisor
                    .get_mut("firecracker")
                    .unwrap()
                    .blockdev_info
                    .block_device_aio = "native".into()
            },
            |c| {
                c.hypervisor
                    .get_mut("firecracker")
                    .unwrap()
                    .blockdev_info
                    .block_device_cache_set = true
            },
            |c| {
                c.hypervisor
                    .get_mut("firecracker")
                    .unwrap()
                    .blockdev_info
                    .block_device_cache_direct = true
            },
            |c| {
                c.hypervisor
                    .get_mut("firecracker")
                    .unwrap()
                    .blockdev_info
                    .block_device_cache_noflush = true
            },
            |c| {
                c.hypervisor
                    .get_mut("firecracker")
                    .unwrap()
                    .blockdev_info
                    .block_device_logical_sector_size = 4096
            },
            |c| {
                c.hypervisor
                    .get_mut("firecracker")
                    .unwrap()
                    .blockdev_info
                    .block_device_physical_sector_size = 4096
            },
            |c| {
                c.hypervisor
                    .get_mut("firecracker")
                    .unwrap()
                    .blockdev_info
                    .num_queues = 4
            },
            |c| {
                c.hypervisor
                    .get_mut("firecracker")
                    .unwrap()
                    .blockdev_info
                    .queue_size = 256
            },
            |c| {
                c.hypervisor
                    .get_mut("firecracker")
                    .unwrap()
                    .blockdev_info
                    .memory_offset = 128
            },
            |c| c.runtime.static_sandbox_resource_mgmt = false,
            |c| c.runtime.disable_new_netns = true,
            |c| c.runtime.enable_vcpus_pinning = true,
            |c| c.runtime.sandbox_cgroup_only = false,
            |c| {
                c.hypervisor
                    .get_mut("firecracker")
                    .unwrap()
                    .boot_info
                    .initrd = "/initrd".into()
            },
            |c| {
                c.hypervisor
                    .get_mut("firecracker")
                    .unwrap()
                    .security_info
                    .rootless = true
            },
            |c| c.runtime.experimental.push("force_guest_pull".into()),
            |c| c.runtime.sandbox_bind_mounts.push("/host".into()),
            |c| c.runtime.use_passfd_io = true,
            |c| c.runtime.emptydir_mode = "block-encrypted".into(),
            |c| c.runtime.enable_pprof = true,
            |c| {
                c.hypervisor
                    .get_mut("firecracker")
                    .unwrap()
                    .security_info
                    .initdata = "data".into()
            },
            |c| c.agent.get_mut("kata").unwrap().policy = "policy".into(),
            |c| {
                c.hypervisor
                    .get_mut("firecracker")
                    .unwrap()
                    .device_info
                    .cold_plug_vfio = "root-port".into()
            },
            |c| {
                c.hypervisor
                    .get_mut("firecracker")
                    .unwrap()
                    .device_info
                    .pcie_root_port = 1
            },
            |c| {
                c.hypervisor
                    .get_mut("firecracker")
                    .unwrap()
                    .device_info
                    .pcie_switch_port = 1
            },
            |c| {
                c.hypervisor
                    .get_mut("firecracker")
                    .unwrap()
                    .jailer_path
                    .clear()
            },
            |c| {
                c.hypervisor
                    .get_mut("firecracker")
                    .unwrap()
                    .security_info
                    .disable_seccomp = true
            },
            |c| {
                c.hypervisor
                    .get_mut("firecracker")
                    .unwrap()
                    .security_info
                    .confidential_guest = true
            },
            |c| {
                c.hypervisor
                    .get_mut("firecracker")
                    .unwrap()
                    .factory
                    .enable_template = true
            },
            |c| {
                c.hypervisor
                    .get_mut("firecracker")
                    .unwrap()
                    .shared_fs
                    .shared_fs = Some("virtio-fs".into())
            },
            |c| {
                c.hypervisor
                    .get_mut("firecracker")
                    .unwrap()
                    .blockdev_info
                    .block_device_driver = "virtio-scsi".into()
            },
            |c| c.agent.get_mut("kata").unwrap().debug_console_enabled = true,
            |c| {
                c.agent
                    .get_mut("kata")
                    .unwrap()
                    .kernel_modules
                    .push("module".into())
            },
        ];
        for mutate in mutations {
            let mut c = supported();
            mutate(&mut c);
            assert!(validate(&c)
                .unwrap_err()
                .to_string()
                .starts_with("kata-fc-minimal:"));
        }
    }
}

/// Kubernetes resource sizing annotations are separate and still handled by CRI.
pub fn validate_annotations(annotations: &std::collections::HashMap<String, String>) -> Result<()> {
    kata_types::annotations::validate_config_annotations(annotations)?;
    Ok(())
}

#[cfg(test)]
mod annotation_tests {
    use super::*;
    #[test]
    fn explicit_annotation_allowlist() {
        for key in [
            "io.katacontainers.config.hypervisor.path",
            "io.katacontainers.config.hypervisor.kernel_params",
            "io.katacontainers.config.agent.debug_console_enabled",
            "io.katacontainers.config.unknown",
        ] {
            assert!(validate_annotations(&[(key.into(), "true".into())].into()).is_err());
        }
        validate_annotations(
            &[(
                "io.katacontainers.config.hypervisor.default_vcpus".into(),
                "-1".into(),
            )]
            .into(),
        )
        .unwrap();
    }
}

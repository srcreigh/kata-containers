// Copyright (c) 2024 Intel Corporation
//
// SPDX-License-Identifier: Apache-2.0
//

/// DRIVER_BLK_PCI_TYPE is the device driver for virtio-blk
pub const DRIVER_BLK_PCI_TYPE: &str = "blk";
/// DRIVER_BLK_CCW_TYPE is the device driver for virtio-blk-ccw
pub const DRIVER_BLK_CCW_TYPE: &str = "blk-ccw";
/// DRIVER_BLK_MMIO_TYPE is the device driver for virtio-mmio
pub const DRIVER_BLK_MMIO_TYPE: &str = "mmioblk";
/// DRIVER_SCSI_TYPE is the device driver for virtio-scsi
pub const DRIVER_SCSI_TYPE: &str = "scsi";
/// DRIVER_NVDIMM_TYPE is the device driver for nvdimm
pub const DRIVER_NVDIMM_TYPE: &str = "nvdimm";
/// DRIVER_VFIO_PCI_GK_TYPE is the device driver for vfio-pci
/// while the device will be bound to a guest kernel driver
pub const DRIVER_VFIO_PCI_GK_TYPE: &str = "vfio-pci-gk";
/// DRIVER_VFIO_PCI_TYPE is the device driver for vfio-pci
/// VFIO PCI device to be bound to vfio-pci and made available inside the
/// container as a VFIO device node
pub const DRIVER_VFIO_PCI_TYPE: &str = "vfio-pci";
/// DRIVER_VFIO_AP_TYPE is the device driver for vfio-ap hotplug.
pub const DRIVER_VFIO_AP_TYPE: &str = "vfio-ap";
/// DRIVER_VFIO_AP_COLD_TYPE is the device driver for vfio-ap coldplug.
pub const DRIVER_VFIO_AP_COLD_TYPE: &str = "vfio-ap-cold";

/// DRIVER_EPHEMERAL_TYPE is the driver for ephemeral volume.
pub const DRIVER_EPHEMERAL_TYPE: &str = "ephemeral";
/// DRIVER_LOCAL_TYPE is the driver for local volume.
pub const DRIVER_LOCAL_TYPE: &str = "local";
/// DRIVER_OVERLAYFS_TYPE is the driver for overlayfs volume.
pub const DRIVER_OVERLAYFS_TYPE: &str = "overlayfs";
/// DRIVER_VIRTIOFS_TYPE is the driver for virtio-fs volume.
pub const DRIVER_VIRTIOFS_TYPE: &str = "virtio-fs";
/// DRIVER_VIRTIOFS_TYPE is the driver for Bind watch volume.
pub const DRIVER_WATCHABLE_BIND_TYPE: &str = "watchable-bind";

/// Registry for the supported guest device handlers.
pub type DeviceHandlerManager<H> = crate::handler::HandlerManager<H>;

/// Reject device integrations excluded from the Firecracker contract. This is
/// shared by the shim (before VM/resource creation) and the guest (before edits).
pub fn validate_spec_device_features(spec: &oci_spec::runtime::Spec) -> anyhow::Result<()> {
    if let Some(annotations) = spec.annotations() {
        anyhow::ensure!(
            !annotations.keys().any(|key| key.starts_with("cdi.k8s.io/")),
            "kata-fc: CDI device annotations are unsupported"
        );
    }
    if let Some(process) = spec.process() {
        for entry in process.env().iter().flatten() {
            anyhow::ensure!(
                !entry
                    .split_once('=')
                    .is_some_and(|(_, value)| value.starts_with("sealed.")),
                "kata-fc: sealed-secret environment values are unsupported"
            );
            if let Some(value) = entry.strip_prefix("VISIBLE_CDI_DEVICES=") {
                anyhow::ensure!(
                    matches!(value.trim(), "" | "none" | "void"),
                    "kata-fc: VISIBLE_CDI_DEVICES requests are unsupported"
                );
            }
        }
    }
    if let Some(linux) = spec.linux() {
        validate_linux_device_features(linux)?;
    }
    Ok(())
}

/// Reject excluded passthrough nodes before attaching any requested device.
pub fn validate_linux_device_features(linux: &oci_spec::runtime::Linux) -> anyhow::Result<()> {
    for device in linux.devices().iter().flatten() {
        validate_device_path(device.path())?;
    }
    Ok(())
}

/// Check both OCI paths and resolved host device paths for excluded integrations.
pub fn validate_device_path(path: &std::path::Path) -> anyhow::Result<()> {
    anyhow::ensure!(
        ![
            "/dev/vfio",
            "/dev/dri",
            "/dev/infiniband",
            "/dev/iommu",
            "/dev/trusted_store"
        ]
        .iter()
        .any(|prefix| path.starts_with(prefix))
            && !path.to_string_lossy().starts_with("/dev/nvidia"),
        "kata-fc: unsupported device path: {}",
        path.display()
    );
    Ok(())
}

#[cfg(test)]
mod minimal_device_tests {
    use super::*;
    use oci_spec::runtime::{Linux, LinuxDevice, Process, Spec};

    #[test]
    fn raw_block_and_character_devices_retained() {
        use oci_spec::runtime::LinuxDeviceType;
        let mut device = LinuxDevice::default();
        device.set_path("/dev/arbitrary-name".into());
        device.set_typ(LinuxDeviceType::B);
        device.set_major(8);
        device.set_minor(0);
        let mut linux = Linux::default();
        linux.set_devices(Some(vec![device.clone()]));
        validate_linux_device_features(&linux).unwrap();
        device.set_path("/dev/null".into());
        device.set_typ(LinuxDeviceType::C);
        device.set_major(1);
        device.set_minor(3);
        linux.set_devices(Some(vec![device]));
        validate_linux_device_features(&linux).unwrap();
    }

    #[test]
    fn minimal_device_contract() {
        let mut spec = Spec::default();
        validate_spec_device_features(&spec).unwrap();
        spec.set_annotations(Some(
            [("cdi.k8s.io/gpu".into(), "nvidia.com/gpu=all".into())].into(),
        ));
        assert!(validate_spec_device_features(&spec).is_err());
        spec.set_annotations(Some(
            [("io.kubernetes.cri.container-name".into(), "worker".into())].into(),
        ));
        let mut process = Process::default();
        for value in ["nvidia.com/gpu=all", "intel.com/gpu=0", "malformed"] {
            process.set_env(Some(vec![format!("VISIBLE_CDI_DEVICES={value}")]));
            spec.set_process(Some(process.clone()));
            assert!(validate_spec_device_features(&spec).is_err());
        }
        process.set_env(Some(vec!["TOKEN=sealed.invalid".into()]));
        spec.set_process(Some(process.clone()));
        assert!(validate_spec_device_features(&spec).is_err());
        for value in ["", "none", "void"] {
            process.set_env(Some(vec![format!("VISIBLE_CDI_DEVICES={value}")]));
            spec.set_process(Some(process.clone()));
            validate_spec_device_features(&spec).unwrap();
        }
        for path in [
            "/dev/vfio/1",
            "/dev/vfio/devices/vfio0",
            "/dev/nvidia0",
            "/dev/dri/renderD128",
            "/dev/infiniband/uverbs0",
            "/dev/iommu",
            "/dev/trusted_store",
        ] {
            let mut linux = Linux::default();
            let mut device = LinuxDevice::default();
            device.set_path(path.into());
            linux.set_devices(Some(vec![device]));
            spec.set_linux(Some(linux));
            assert!(validate_spec_device_features(&spec).is_err(), "{}", path);
        }
        for path in ["/dev/null", "/dev/random", "/dev/fuse", "/dev/vdb"] {
            validate_device_path(std::path::Path::new(path)).unwrap();
        }
    }
}

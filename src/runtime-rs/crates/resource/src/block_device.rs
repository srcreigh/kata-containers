// Copyright Kata Contributors
// SPDX-License-Identifier: Apache-2.0

use anyhow::{ensure, Result};
use hypervisor::{BlockConfigModern, KATA_MMIO_BLK_DEV_TYPE};

/// MMIO disks are addressed by their guest device path, never a PCI/SCSI address.
pub(crate) fn agent_storage_source_from_block_config(config: &BlockConfigModern) -> Result<String> {
    ensure!(
        config.driver_option == KATA_MMIO_BLK_DEV_TYPE,
        "unsupported block driver {}",
        config.driver_option
    );
    ensure!(
        !config.virt_path.is_empty(),
        "MMIO block device has no guest path"
    );
    Ok(config.virt_path.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mmio_requires_guest_path() {
        let mut config = BlockConfigModern {
            driver_option: KATA_MMIO_BLK_DEV_TYPE.into(),
            ..Default::default()
        };
        assert!(agent_storage_source_from_block_config(&config).is_err());
        config.virt_path = "/dev/vdd".into();
        assert_eq!(
            agent_storage_source_from_block_config(&config).unwrap(),
            "/dev/vdd"
        );
    }

    #[test]
    fn alternate_transports_are_rejected_even_with_a_guest_path() {
        for driver in ["blk", "scsi", "ccw", "nvdimm", "virtio-blk-pci", "unknown"] {
            let config = BlockConfigModern {
                driver_option: driver.into(),
                virt_path: "/dev/vdd".into(),
                ..Default::default()
            };
            assert!(agent_storage_source_from_block_config(&config)
                .unwrap_err()
                .to_string()
                .contains("unsupported block driver"));
        }
    }
}

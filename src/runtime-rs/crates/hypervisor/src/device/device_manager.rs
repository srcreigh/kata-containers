// Copyright (c) 2019-2023 Alibaba Cloud
// Copyright (c) 2019-2023 Ant Group
//
// SPDX-License-Identifier: Apache-2.0
//

use std::{collections::HashMap, sync::Arc};

use anyhow::{anyhow, Context, Result};
use kata_sys_util::rand::RandomBytes;
use kata_types::config::hypervisor::{BlockDeviceInfo, VIRTIO_SCSI};
use tokio::sync::{Mutex, RwLock};

use crate::{
    BlockConfigModern, BlockDeviceModernHandle, HybridVsockDevice, Hypervisor, NetworkDevice,
    KATA_BLK_DEV_TYPE, KATA_CCW_DEV_TYPE, KATA_MMIO_BLK_DEV_TYPE, KATA_NVDIMM_DEV_TYPE,
    KATA_SCSI_DEV_TYPE, VIRTIO_BLOCK_CCW, VIRTIO_BLOCK_MMIO, VIRTIO_BLOCK_PCI, VIRTIO_PMEM,
};

use super::{
    util::{get_host_path, get_virt_drive_name, DEVICE_TYPE_BLOCK},
    Device, DeviceConfig, DeviceType,
};

pub type ArcMutexDevice = Arc<Mutex<dyn Device>>;

macro_rules! declare_index {
    ($self:ident, $index:ident, $released_index:ident) => {{
        let current_index = if let Some(index) = $self.$released_index.pop() {
            index
        } else {
            $self.$index
        };
        $self.$index += 1;
        Ok(current_index)
    }};
}

macro_rules! release_index {
    ($self:ident, $index:ident, $released_index:ident) => {{
        $self.$released_index.push($index);
        $self.$released_index.sort_by(|a, b| b.cmp(a));
    }};
}

/// block_index and released_block_index are used to search an available block index
/// in Sandbox.
/// pmem_index and released_pmem_index are used to search an available pmem index
/// in Sandbox.
///
/// @pmem_index generally default is 0 for <pmem0>;
/// @block_index generally default is 0 for <vda>;
/// @released_pmem_index for pmem devices removed and indexes will released at the same time.
/// @released_block_index for blk devices removed and indexes will released at the same time.
#[derive(Clone, Debug, Default)]
struct SharedInfo {
    pmem_index: u64,
    block_index: u64,
    released_pmem_index: Vec<u64>,
    released_block_index: Vec<u64>,
}

impl SharedInfo {
    async fn new() -> Self {
        SharedInfo {
            pmem_index: 0,
            block_index: 0,
            released_pmem_index: vec![],
            released_block_index: vec![],
        }
    }

    fn declare_device_index(&mut self, is_pmem: bool) -> Result<u64> {
        if is_pmem {
            declare_index!(self, pmem_index, released_pmem_index)
        } else {
            declare_index!(self, block_index, released_block_index)
        }
    }

    fn release_device_index(&mut self, index: u64, is_pmem: bool) {
        if is_pmem {
            release_index!(self, index, released_pmem_index);
        } else {
            release_index!(self, index, released_block_index);
        }
    }
}

// Device manager will manage the lifecycle of sandbox device
#[derive(Debug)]
pub struct DeviceManager {
    devices: HashMap<String, ArcMutexDevice>,
    hypervisor: Arc<dyn Hypervisor>,
    shared_info: SharedInfo,
}

impl DeviceManager {
    pub async fn new(hypervisor: Arc<dyn Hypervisor>) -> Result<Self> {
        let devices = HashMap::<String, ArcMutexDevice>::new();
        Ok(DeviceManager {
            devices,
            hypervisor,
            shared_info: SharedInfo::new().await,
        })
    }

    async fn get_block_device_info(&self) -> BlockDeviceInfo {
        self.hypervisor.hypervisor_config().await.blockdev_info
    }

    async fn try_add_device(&mut self, device_id: &str) -> Result<()> {
        // find the device
        let device = self
            .devices
            .get(device_id)
            .context("failed to find device")?;

        let mut device_guard = device.lock().await;
        // attach device
        let result = device_guard.attach(self.hypervisor.as_ref()).await;
        // handle attach error
        if let Err(e) = result {
            match device_guard.get_device_info().await {
                DeviceType::BlockModern(device) => {
                    let (index, is_pmem) = {
                        let cfg = &device.lock().await.config;
                        (cfg.index, cfg.driver_option == *KATA_NVDIMM_DEV_TYPE)
                    };
                    self.shared_info.release_device_index(index, is_pmem);
                }
                _ => {
                    debug!(sl!(), "no need to do release device index.");
                }
            }

            drop(device_guard);
            self.devices.remove(device_id);

            return Err(e);
        }

        Ok(())
    }

    pub async fn try_remove_device(&mut self, device_id: &str) -> Result<()> {
        if let Some(dev) = self.devices.get(device_id) {
            let mut device_guard = dev.lock().await;
            let result = match device_guard.detach(self.hypervisor.as_ref()).await {
                Ok(index) => {
                    if let Some(i) = index {
                        // release the declared device index
                        let is_pmem = match device_guard.get_device_info().await {
                            DeviceType::BlockModern(dev) => {
                                dev.lock().await.config.driver_option == *KATA_NVDIMM_DEV_TYPE
                            }
                            _ => false,
                        };
                        self.shared_info.release_device_index(i, is_pmem);
                    }
                    Ok(())
                }
                Err(e) => Err(e),
            };

            // if detach success, remove it from device manager
            if result.is_ok() {
                drop(device_guard);
                self.devices.remove(device_id);
            }

            return result;
        }

        Err(anyhow!(
            "device with specified ID hasn't been created. {}",
            device_id
        ))
    }

    async fn get_device_info(&self, device_id: &str) -> Result<DeviceType> {
        if let Some(dev) = self.devices.get(device_id) {
            return Ok(dev.lock().await.get_device_info().await);
        }

        Err(anyhow!(
            "device with specified ID hasn't been created. {}",
            device_id
        ))
    }

    async fn find_device(&self, host_path: String) -> Option<String> {
        for (device_id, dev) in &self.devices {
            match dev.lock().await.get_device_info().await {
                DeviceType::Network(device) => {
                    if device.config.host_dev_name == host_path {
                        return Some(device_id.to_string());
                    }
                }

                DeviceType::BlockModern(device) => {
                    if device.lock().await.config.path_on_host == host_path {
                        return Some(device_id.to_string());
                    }
                }
                DeviceType::HybridVsock(_) => {
                    continue;
                }
            }
        }

        None
    }

    fn get_dev_virt_path(
        &mut self,
        dev_type: &str,
        is_pmem: bool,
    ) -> Result<Option<(u64, String)>> {
        let virt_path = if dev_type == DEVICE_TYPE_BLOCK {
            let current_index = self.shared_info.declare_device_index(is_pmem)?;
            let drive_name = if is_pmem {
                format!("pmem{current_index}")
            } else {
                get_virt_drive_name(current_index as i32)?
            };
            let virt_path_name = format!("/dev/{drive_name}");
            Some((current_index, virt_path_name))
        } else {
            // only dev_type is block, otherwise, it's None.
            None
        };

        Ok(virt_path)
    }

    async fn new_device(&mut self, device_config: &DeviceConfig) -> Result<String> {
        // device ID must be generated by manager instead of device itself
        // in case of ID collision
        let device_id = self.new_device_id()?;
        let dev: ArcMutexDevice = match device_config {
            DeviceConfig::BlockCfgModern(config) => {
                anyhow::ensure!(
                    config.driver_option == VIRTIO_BLOCK_MMIO,
                    "kata-fc-minimal: unsupported block transport {}",
                    config.driver_option
                );

                if let Some(device_matched_id) = self.find_device(config.path_on_host.clone()).await
                {
                    return Ok(device_matched_id);
                }

                self.create_block_device_modern(config, device_id.clone())
                    .await
                    .context("failed to create block device modern")?
            }

            DeviceConfig::NetworkCfg(config) => {
                // try to find the device, found and just return id.
                let host_path = config.host_dev_name.as_str();
                if let Some(dev_id_matched) = self.find_device(host_path.to_owned()).await {
                    info!(
                        sl!(),
                        "network device with path:{:?} found. return network device id: {:?}",
                        host_path,
                        dev_id_matched
                    );

                    return Ok(dev_id_matched);
                }

                Arc::new(Mutex::new(NetworkDevice::new(device_id.clone(), config)))
            }

            DeviceConfig::HybridVsockCfg(hvconfig) => {
                // No need to do find device for hybrid vsock device.
                Arc::new(Mutex::new(HybridVsockDevice::new(&device_id, hvconfig)))
            }
        };

        // register device to devices
        self.devices.insert(device_id.clone(), dev.clone());

        Ok(device_id)
    }

    async fn create_block_device_modern(
        &mut self,
        config: &BlockConfigModern,
        device_id: String,
    ) -> Result<ArcMutexDevice> {
        let mut block_config = config.clone();
        let mut is_pmem = false;

        match block_config.driver_option.as_str() {
            VIRTIO_BLOCK_MMIO => {
                block_config.driver_option = KATA_MMIO_BLK_DEV_TYPE.to_string();
            }
            VIRTIO_BLOCK_PCI => {
                block_config.driver_option = KATA_BLK_DEV_TYPE.to_string();
            }
            VIRTIO_BLOCK_CCW => {
                block_config.driver_option = KATA_CCW_DEV_TYPE.to_string();
            }
            VIRTIO_PMEM => {
                block_config.driver_option = KATA_NVDIMM_DEV_TYPE.to_string();
                is_pmem = true;
            }
            VIRTIO_SCSI => {
                block_config.driver_option = KATA_SCSI_DEV_TYPE.to_string();
            }
            _ => {
                return Err(anyhow!(
                    "unsupported driver type {}",
                    block_config.driver_option
                ));
            }
        };

        if let Some(virt_path) = self.get_dev_virt_path(DEVICE_TYPE_BLOCK, is_pmem)? {
            block_config.index = virt_path.0;
            block_config.virt_path = virt_path.1;
        }

        if block_config.path_on_host.is_empty() {
            block_config.path_on_host =
                get_host_path(DEVICE_TYPE_BLOCK, config.major, config.minor)
                    .context("failed to get host path")?;
        }

        Ok(Arc::new(Mutex::new(BlockDeviceModernHandle::new(
            device_id,
            block_config,
        ))))
    }

    // device ID must be generated by device manager instead of device itself
    // in case of ID collision
    fn new_device_id(&self) -> Result<String> {
        for _ in 0..5 {
            let rand_bytes = RandomBytes::new(8);
            let id = format!("{rand_bytes:x}");

            // check collision in devices
            if !self.devices.contains_key(&id) {
                return Ok(id);
            }
        }

        Err(anyhow!("ID are exhausted"))
    }
}

// Many scenarios have similar steps when adding devices. so to reduce duplicated code,
// we should create a common method abstracted and use it in various scenarios.
// do_handle_device:
// (1) new_device with DeviceConfig and return device_id;
// (2) try_add_device with device_id and do really add device;
// (3) return device info of device's info;
pub async fn do_handle_device(
    d: &RwLock<DeviceManager>,
    dev_info: &DeviceConfig,
) -> Result<DeviceType> {
    let device_id = d
        .write()
        .await
        .new_device(dev_info)
        .await
        .context("failed to create device")?;

    d.write()
        .await
        .try_add_device(&device_id)
        .await
        .context("failed to add device")?;

    let device_info = d
        .read()
        .await
        .get_device_info(&device_id)
        .await
        .context("failed to get device info")?;

    Ok(device_info)
}

pub async fn get_block_device_info(d: &RwLock<DeviceManager>) -> BlockDeviceInfo {
    d.read().await.get_block_device_info().await
}

#[cfg(test)]
mod tests {
    use super::DeviceManager;
    use crate::{
        device::{device_manager::get_block_device_info, DeviceConfig, DeviceType},
        firecracker::Firecracker,
        BlockConfigModern, KATA_MMIO_BLK_DEV_TYPE,
    };
    use anyhow::{Context, Result};
    use std::sync::Arc;
    use tokio::sync::RwLock;

    async fn new_device_manager() -> Result<Arc<RwLock<DeviceManager>>> {
        let mut config = kata_types::config::Hypervisor::default();
        config.blockdev_info.block_device_driver = "virtio-blk-mmio".into();
        let hypervisor = Firecracker::new();
        hypervisor.set_hypervisor_config(config).await;

        let dm = Arc::new(RwLock::new(
            DeviceManager::new(Arc::new(hypervisor))
                .await
                .context("device manager")?,
        ));

        Ok(dm)
    }

    #[actix_rt::test]
    async fn test_new_block_device() {
        let dm = new_device_manager().await;
        assert!(dm.is_ok());

        let d = dm.unwrap();
        let block_driver = get_block_device_info(&d).await.block_device_driver;
        let dev_info = DeviceConfig::BlockCfgModern(BlockConfigModern {
            path_on_host: "/dev/dddzzz".to_string(),
            driver_option: block_driver,
            ..Default::default()
        });
        let new_device_result = d.write().await.new_device(&dev_info).await;
        assert!(new_device_result.is_ok());

        let device_id = new_device_result.unwrap();
        let devices_info_result = d.read().await.get_device_info(&device_id).await;
        assert!(devices_info_result.is_ok());

        let device_info = devices_info_result.unwrap();
        if let DeviceType::BlockModern(device_lock) = device_info {
            let device = device_lock.lock().await;
            assert_eq!(device.config.driver_option, KATA_MMIO_BLK_DEV_TYPE);
        } else {
            panic!("expected an MMIO block device")
        }
    }

    #[tokio::test]
    async fn unsupported_transports_do_not_enter_manager() {
        let dm = new_device_manager().await.unwrap();
        let mut dm = dm.write().await;
        for driver in [
            "virtio-blk-pci",
            "virtio-blk-ccw",
            "virtio-scsi",
            "virtio-pmem",
        ] {
            let config = DeviceConfig::BlockCfgModern(BlockConfigModern {
                driver_option: driver.into(),
                path_on_host: "/dev/unsupported".into(),
                ..Default::default()
            });
            assert!(dm
                .new_device(&config)
                .await
                .unwrap_err()
                .to_string()
                .contains("unsupported block transport"));
            assert!(dm.devices.is_empty());
            assert_eq!(dm.shared_info.block_index, 0);
        }
    }
}

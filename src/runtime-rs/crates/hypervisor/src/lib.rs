// Copyright (c) 2019-2022 Alibaba Cloud
// Copyright (c) 2019-2022 Ant Group
//
// SPDX-License-Identifier: Apache-2.0
//

#[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
compile_error!("kata-fc-minimal supports only x86_64 Linux");

#[macro_use]
extern crate slog;

logging::logger_with_subsystem!(sl, "hypervisor");

pub mod device;
pub mod hypervisor_persist;
pub use device::driver::*;
use device::DeviceType;

pub mod firecracker;
mod kernel_param;

pub mod selinux;
pub use kernel_param::Param;
use std::collections::HashMap;

use anyhow::Result;
use async_trait::async_trait;
use hypervisor_persist::HypervisorState;
use kata_types::config::hypervisor::Hypervisor as HypervisorConfig;

// Config which driver to use as vm root dev
const VM_ROOTFS_DRIVER_MMIO: &str = "virtio-blk-mmio";

//Configure the root corresponding to the driver
const VM_ROOTFS_ROOT_BLK: &str = "/dev/vda1";

// before using hugepages for VM, we need to mount hugetlbfs
// /dev/hugepages will be the mount point
// mkdir -p /dev/hugepages
// mount -t hugetlbfs none /dev/hugepages
pub const HUGETLBFS: &str = "hugetlbfs";
pub const HYPERVISOR_FIRECRACKER: &str = "firecracker";

#[derive(PartialEq, Debug, Clone)]
pub(crate) enum VmmState {
    NotReady,
    VmmServerReady,
    VmRunning,
}

#[async_trait]
pub trait Hypervisor: std::fmt::Debug + Send + Sync {
    // vm manager
    async fn prepare_vm(
        &self,
        id: &str,
        netns: Option<String>,
        annotations: &HashMap<String, String>,
        selinux_label: Option<String>,
    ) -> Result<()>;
    async fn start_vm(&self, timeout: i32) -> Result<()>;
    async fn stop_vm(&self) -> Result<()>;
    async fn wait_vm(&self) -> Result<i32>;

    // device manager
    async fn add_device(&self, device: DeviceType) -> Result<()>;

    // utils
    async fn get_agent_socket(&self) -> Result<String>;
    async fn hypervisor_config(&self) -> HypervisorConfig;
    async fn get_vmm_master_tid(&self) -> Result<u32>;
    async fn cleanup(&self) -> Result<()>;
    async fn save_state(&self) -> Result<HypervisorState>;
}

// Copyright (c) 2019-2022 Alibaba Cloud
// Copyright (c) 2019-2022 Ant Group
//
// SPDX-License-Identifier: Apache-2.0
//

#[macro_use]
extern crate slog;

logging::logger_with_subsystem!(sl, "resource");

mod block_device;
pub mod cgroups;
pub mod manager;
mod manager_inner;
pub mod network;
pub mod resource_persist;
use hypervisor::BlockConfigModern;
use network::NetworkConfig;
mod guest_paths;
pub mod rootfs;
pub mod volume;
pub use manager::ResourceManager;
pub mod cpu_mem;

#[derive(Debug)]
pub enum ResourceConfig {
    Network(NetworkConfig),
    VmRootfs(BlockConfigModern),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ResourceUpdateOp {
    Add,
    Del,
    Update,
}

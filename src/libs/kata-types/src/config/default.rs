// Copyright (c) 2021 Alibaba Cloud
//
// SPDX-License-Identifier: Apache-2.0
//

//! Default configuration values.
#![allow(missing_docs)]

use lazy_static::lazy_static;

lazy_static! {
    /// Default configuration file paths, vendor may extend the list
    pub static ref DEFAULT_RUNTIME_CONFIGURATIONS: Vec::<&'static str> = vec![
        // The rust runtime specific paths
        "/etc/kata-containers/runtime-rs/configuration.toml",
        "/usr/share/defaults/kata-containers/runtime-rs/configuration.toml",
        "/opt/kata/share/defaults/kata-containers/runtime-rs/configuration.toml",
    ];
}

pub const DEFAULT_AGENT_VSOCK_PORT: u32 = 1024;
pub const DEFAULT_AGENT_LOG_PORT: u32 = 1025;
pub const DEFAULT_PASSFD_LISTENER_PORT: u32 = 1027;
pub const DEFAULT_AGENT_DIAL_TIMEOUT_MS: u32 = 10;

pub const DEFAULT_INTERNETWORKING_MODEL: &str = "tcfilter";

pub const DEFAULT_BLOCK_DEVICE_TYPE: &str = "virtio-blk-mmio";
pub const DEFAULT_VHOST_USER_STORE_PATH: &str = "/var/run/vhost-user";
pub const DEFAULT_BLOCK_NVDIMM_MEM_OFFSET: u64 = 0;
pub const DEFAULT_BLOCK_DEVICE_AIO_THREADS: &str = "threads";
pub const DEFAULT_BLOCK_DEVICE_AIO_NATIVE: &str = "native";
pub const DEFAULT_BLOCK_DEVICE_AIO: &str = "io_uring";
pub const DEFAULT_BLOCK_DEVICE_NUM_QUEUES: u32 = 1;
pub const DEFAULT_BLOCK_DEVICE_QUEUE_SIZE: u32 = 128;

pub const DEFAULT_GUEST_DNS_FILE: &str = "/etc/resolv.conf";

pub const DEFAULT_GUEST_VCPUS: u32 = 1;

//Default configuration for firecracker
pub const DEFAULT_FIRECRACKER_ENTROPY_SOURCE: &str = "/dev/urandom";
pub const DEFAULT_FIRECRACKER_MEMORY_SIZE_MB: u32 = 128;
pub const DEFAULT_FIRECRACKER_GUEST_KERNEL_IMAGE: &str = "vmlinux";
pub const DEFAULT_FIRECRACKER_GUEST_KERNEL_PARAMS: &str = "";
pub const MAX_FIRECRACKER_VCPUS: u32 = 32;
pub const MIN_FIRECRACKER_MEMORY_SIZE_MB: u32 = 128;

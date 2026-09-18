// Copyright (c) 2019-2023 Alibaba Cloud
// Copyright (c) 2019-2023 Ant Group
//
// SPDX-License-Identifier: Apache-2.0
//

pub mod virtio_blk_modern;
mod virtio_net;
mod virtio_vsock;

pub use kata_types::device::{
    DRIVER_BLK_CCW_TYPE as KATA_CCW_DEV_TYPE, DRIVER_BLK_MMIO_TYPE as KATA_MMIO_BLK_DEV_TYPE,
    DRIVER_BLK_PCI_TYPE as KATA_BLK_DEV_TYPE, DRIVER_NVDIMM_TYPE as KATA_NVDIMM_DEV_TYPE,
    DRIVER_SCSI_TYPE as KATA_SCSI_DEV_TYPE,
};
pub use virtio_blk_modern::{
    BlockConfigModern, BlockDeviceAio, BlockDeviceModern, BlockDeviceModernHandle, VmdkConfig,
    VmdkExtent, VIRTIO_BLOCK_CCW, VIRTIO_BLOCK_MMIO, VIRTIO_BLOCK_PCI, VIRTIO_PMEM,
};
pub use virtio_net::{Address, NetworkConfig, NetworkDevice};
pub use virtio_vsock::{HybridVsockConfig, HybridVsockDevice, DEFAULT_GUEST_VSOCK_CID};

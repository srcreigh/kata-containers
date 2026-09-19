// Copyright (c) 2019-2023 Alibaba Cloud
// Copyright (c) 2019-2023 Ant Group
//
// SPDX-License-Identifier: Apache-2.0
//

pub mod virtio_blk_modern;
mod virtio_net;

pub use kata_types::device::DRIVER_BLK_MMIO_TYPE as KATA_MMIO_BLK_DEV_TYPE;
pub use virtio_blk_modern::{
    BlockConfigModern, BlockDeviceModern, BlockDeviceModernHandle, VIRTIO_BLOCK_MMIO,
};
pub use virtio_net::{Address, NetworkConfig, NetworkDevice};

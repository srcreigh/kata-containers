// Copyright (c) 2019-2022 Alibaba Cloud
// Copyright (c) 2019-2022 Ant Group
//
// SPDX-License-Identifier: Apache-2.0
//

mod create;
pub use create::create_tap;
mod driver_info;
pub use driver_info::get_driver_info;
mod macros;
mod manager;
pub use manager::get_link_from_message;

#[cfg(test)]
pub use create::net_test_utils;

#[derive(Debug, PartialEq, Eq, Clone, Default)]
pub struct LinkAttrs {
    pub index: u32,
    pub mtu: u32,
    pub name: String,
    pub hardware_addr: Vec<u8>,
    pub flags: u32,
}

pub trait Link: Send + Sync {
    fn attrs(&self) -> &LinkAttrs;
    fn set_attrs(&mut self, attr: LinkAttrs);
    fn r#type(&self) -> &str;
}

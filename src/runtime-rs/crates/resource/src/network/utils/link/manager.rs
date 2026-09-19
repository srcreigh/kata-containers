// Copyright (c) 2019-2022 Alibaba Cloud
// Copyright (c) 2019-2022 Ant Group
//
// SPDX-License-Identifier: Apache-2.0
//

use netlink_packet_route::link::{InfoData, InfoKind, LinkAttribute, LinkInfo, LinkMessage};

use super::{Link, LinkAttrs};

#[allow(clippy::box_default)]
pub fn get_link_from_message(mut msg: LinkMessage) -> Box<dyn Link> {
    let flags = msg.header.flags.bits();

    let mut base = LinkAttrs {
        index: msg.header.index,
        flags,
        ..Default::default()
    };
    let mut link: Option<Box<dyn Link>> = None;
    while let Some(attr) = msg.attributes.pop() {
        match attr {
            LinkAttribute::LinkInfo(infos) => {
                link = Some(link_info(infos));
            }
            LinkAttribute::Address(a) => {
                base.hardware_addr = a;
            }
            LinkAttribute::IfName(i) => {
                base.name = i;
            }
            LinkAttribute::Mtu(m) => {
                base.mtu = m;
            }
            _ => {
                // skip unused attr
            }
        }
    }

    let mut ret = link.unwrap_or_else(|| Box::new(Device::default()));
    ret.set_attrs(base);
    ret
}

#[allow(clippy::box_default)]
fn link_info(mut infos: Vec<LinkInfo>) -> Box<dyn Link> {
    let mut link: Option<Box<dyn Link>> = None;
    while let Some(info) = infos.pop() {
        match info {
            LinkInfo::Kind(kind) => match kind {
                InfoKind::Tun => {
                    if link.is_none() {
                        link = Some(Box::new(Tuntap::default()));
                    }
                }
                InfoKind::Veth => {
                    if link.is_none() {
                        link = Some(Box::new(Veth::default()));
                    }
                }
                InfoKind::IpVlan => {
                    if link.is_none() {
                        link = Some(Box::new(IpVlan::default()));
                    }
                }
                InfoKind::MacVlan => {
                    if link.is_none() {
                        link = Some(Box::new(MacVlan::default()));
                    }
                }
                InfoKind::Vlan => {
                    if link.is_none() {
                        link = Some(Box::new(Vlan::default()));
                    }
                }
                InfoKind::Bridge => {
                    if link.is_none() {
                        link = Some(Box::new(Bridge::default()));
                    }
                }
                _ => {
                    if link.is_none() {
                        link = Some(Box::new(Device::default()));
                    }
                }
            },
            LinkInfo::Data(data) => match data {
                InfoData::Tun(_) => {
                    link = Some(Box::new(Tuntap::default()));
                }
                InfoData::Veth(_) => {
                    link = Some(Box::new(Veth::default()));
                }
                InfoData::IpVlan(_) => {
                    link = Some(Box::new(IpVlan::default()));
                }
                InfoData::MacVlan(_) => {
                    link = Some(Box::new(MacVlan::default()));
                }
                InfoData::Vlan(_) => {
                    link = Some(Box::new(Vlan::default()));
                }
                InfoData::Bridge(_) => {
                    link = Some(Box::new(Bridge::default()));
                }
                _ => {
                    link = Some(Box::new(Device::default()));
                }
            },
            LinkInfo::PortKind(_sk) => {
                if link.is_none() {
                    link = Some(Box::new(Device::default()));
                }
            }
            LinkInfo::PortData(_sd) => {
                link = Some(Box::new(Device::default()));
            }
            _ => {
                link = Some(Box::new(Device::default()));
            }
        }
    }
    link.unwrap()
}

macro_rules! impl_network_dev {
    ($r_type: literal , $r_struct: ty) => {
        impl Link for $r_struct {
            fn attrs(&self) -> &LinkAttrs {
                self.attrs.as_ref().unwrap()
            }
            fn set_attrs(&mut self, attr: LinkAttrs) {
                self.attrs = Some(attr);
            }
            fn r#type(&self) -> &'static str {
                $r_type
            }
        }
    };
}

macro_rules! define_and_impl_network_dev {
    ($r_type: literal , $r_struct: tt) => {
        #[derive(Debug, PartialEq, Eq, Clone, Default)]
        pub struct $r_struct {
            attrs: Option<LinkAttrs>,
        }

        impl_network_dev!($r_type, $r_struct);
    };
}

define_and_impl_network_dev!("device", Device);
define_and_impl_network_dev!("tuntap", Tuntap);
define_and_impl_network_dev!("veth", Veth);
define_and_impl_network_dev!("ipvlan", IpVlan);
define_and_impl_network_dev!("macvlan", MacVlan);
define_and_impl_network_dev!("vlan", Vlan);

define_and_impl_network_dev!("bridge", Bridge);

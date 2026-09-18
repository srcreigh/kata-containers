// Copyright (c) 2019-2022 Alibaba Cloud
// Copyright (c) 2019-2022 Ant Group
//
// SPDX-License-Identifier: Apache-2.0
//

use std::convert::TryFrom;
use std::net::{IpAddr, Ipv4Addr};

use anyhow::Result;
use netlink_packet_route::address::AddressAttribute;
use netlink_packet_route::address::AddressMessage;

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Address {
    pub addr: IpAddr,
    pub label: String,
    pub flags: u32,
    pub scope: u8,
    pub perfix_len: u8,
    pub peer: IpAddr,
    pub broadcast: IpAddr,
    pub prefered_lft: u32,
    pub valid_ltf: u32,
}

impl TryFrom<AddressMessage> for Address {
    type Error = anyhow::Error;
    fn try_from(msg: AddressMessage) -> Result<Self> {
        let AddressMessage {
            header, attributes, ..
        } = msg;
        let mut addr = Address {
            addr: IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)),
            peer: IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)),
            broadcast: IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)),
            label: String::default(),
            flags: 0,
            scope: u8::from(header.scope),
            perfix_len: header.prefix_len,
            prefered_lft: 0,
            valid_ltf: 0,
        };

        for nla in attributes.into_iter() {
            match nla {
                AddressAttribute::Address(a) => {
                    addr.addr = a;
                }
                AddressAttribute::Broadcast(b) => {
                    addr.broadcast = IpAddr::V4(b);
                }
                AddressAttribute::Label(l) => {
                    addr.label = l;
                }
                AddressAttribute::Flags(f) => {
                    //since the AddressAttribute::Flags(f) didn't implemented the u32 from trait,
                    //thus here just implemeted a simple transformer.
                    addr.flags = f.bits();
                }
                AddressAttribute::CacheInfo(_c) => {}
                _ => {}
            }
        }

        Ok(addr)
    }
}

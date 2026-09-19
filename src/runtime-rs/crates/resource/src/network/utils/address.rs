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
    pub perfix_len: u8,
}

impl TryFrom<AddressMessage> for Address {
    type Error = anyhow::Error;
    fn try_from(msg: AddressMessage) -> Result<Self> {
        let AddressMessage {
            header, attributes, ..
        } = msg;
        let mut addr = Address {
            addr: IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)),
            perfix_len: header.prefix_len,
        };

        for nla in attributes.into_iter() {
            match nla {
                AddressAttribute::Address(a) => {
                    addr.addr = a;
                }
                _ => {}
            }
        }

        Ok(addr)
    }
}

// Copyright 2018 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//
// Portions Copyright 2017 The Chromium OS Authors. All rights reserved.
// See THIRD-PARTY for the Chromium BSD license.
// Retained from dbs-utils: Firecracker only needs MAC serialization.

use serde::{Serialize, Serializer};
use std::fmt;

pub(super) struct MacAddr(pub(super) [u8; 6]);

impl fmt::Display for MacAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let b = &self.0;
        write!(
            f,
            "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            b[0], b[1], b[2], b[3], b[4], b[5]
        )
    }
}

impl Serialize for MacAddr {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn firecracker_mac_json_preserves_octets() {
        let mac = MacAddr([0, 1, 0x0f, 0x80, 0xab, 0xff]);
        assert_eq!(
            serde_json::to_string(&mac).unwrap(),
            "\"00:01:0f:80:ab:ff\""
        );
        assert_eq!(
            serde_json::to_string(&Option::<MacAddr>::None).unwrap(),
            "null"
        );
    }
}

// Copyright (c) 2019-2022 Alibaba Cloud
// Copyright (c) 2019-2022 Ant Group
//
// SPDX-License-Identifier: Apache-2.0
//

use serde::{Deserialize, Serialize};

use crate::cgroups::cgroup_persist::CgroupState;
#[derive(Serialize, Deserialize, Default)]
pub struct ResourceState {
    pub cgroup_state: Option<CgroupState>,
}

#[cfg(test)]
mod tests {
    #[test]
    fn legacy_unused_network_state_is_ignored() {
        let state: super::ResourceState = serde_json::from_str(
            r#"{"endpoint":[{"veth_endpoint":{"if_name":"eth0","network_qos":false}}],"cgroup_state":null}"#
        ).unwrap();
        assert!(state.cgroup_state.is_none());
    }
}

// Copyright (c) 2019-2022 Alibaba Cloud
// Copyright (c) 2019-2022 Ant Group
//
// SPDX-License-Identifier: Apache-2.0
//
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
pub struct CgroupState {
    pub path: Option<String>,
    // Retained only to reject unsupported modes in older saved state.
    // New state always writes true/false respectively.
    pub sandbox_cgroup_only: bool,
    pub enable_vcpus_pinning: bool,
}

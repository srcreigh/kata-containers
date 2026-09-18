// Copyright (c) 2022 Alibaba Cloud
// Copyright (c) 2024 Ant Group
// SPDX-License-Identifier: Apache-2.0

//! Constants shared by the retained shim management server.
pub mod shim_mgmt;

pub const SHIM_MGMT_SOCK_NAME: &str = "shim-monitor.sock";

pub fn sb_storage_path() -> &'static str {
    kata_types::config::KATA_PATH
}

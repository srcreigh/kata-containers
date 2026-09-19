// Copyright (c) 2024 Ant Group
//
// SPDX-License-Identifier: Apache-2.0
//

pub const OCI_SPEC_CONFIG_FILE_NAME: &str = "config.json";

#[derive(Debug, Copy, Clone, PartialEq)]
pub enum ContainerState {
    Created,
    Running,
    Stopped,
    Paused,
}

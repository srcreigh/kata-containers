// Copyright (c) 2019-2022 Alibaba Cloud
// Copyright (c) 2019-2022 Ant Group
//
// SPDX-License-Identifier: Apache-2.0

use crate::{ContainerManager, Sandbox};
use std::sync::Arc;

#[derive(Clone)]
pub struct RuntimeInstance {
    pub sandbox: Arc<dyn Sandbox>,
    pub container_manager: Arc<dyn ContainerManager>,
}

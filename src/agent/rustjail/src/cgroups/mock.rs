// Copyright (c) 2020 Intel Corporation
//
// SPDX-License-Identifier: Apache-2.0
//

use protobuf::MessageField;

use crate::cgroups::Manager as CgroupManager;
use crate::cgroups_rs as cgroups;
use crate::protocols::agent::{BlkioStats, CgroupStats, CpuStats, MemoryStats, PidsStats};
use anyhow::Result;
use cgroups::freezer::FreezerState;
use libc::{self, pid_t};
use oci::{LinuxResources, Spec};
use oci_spec::runtime as oci;
use std::collections::HashMap;
use std::string::String;
use std::sync::{Arc, RwLock};

use super::DevicesCgroupInfo;

#[derive(Debug, Clone)]
pub struct Manager;

impl CgroupManager for Manager {
    fn apply(&self, _: pid_t) -> Result<()> {
        Ok(())
    }

    fn set(&self, _: &LinuxResources) -> Result<()> {
        Ok(())
    }

    fn get_stats(&self) -> Result<CgroupStats> {
        Ok(CgroupStats {
            cpu_stats: MessageField::some(CpuStats::default()),
            memory_stats: MessageField::some(MemoryStats::new()),
            pids_stats: MessageField::some(PidsStats::new()),
            blkio_stats: MessageField::some(BlkioStats::new()),
            hugetlb_stats: HashMap::new(),
            ..Default::default()
        })
    }

    fn freeze(&self, _: FreezerState) -> Result<()> {
        Ok(())
    }

    fn destroy(&self) -> Result<()> {
        Ok(())
    }

    fn get_pids(&self) -> Result<Vec<pid_t>> {
        Ok(Vec::new())
    }

    fn get_cgroup_path(&self) -> Result<String> {
        Ok("".to_string())
    }

    fn name(&self) -> &str {
        "mock"
    }
}

impl Manager {
    pub fn new(
        _cpath: &str,
        _spec: &Spec,
        _devcg_info: Option<Arc<RwLock<DevicesCgroupInfo>>>,
    ) -> Result<Self> {
        Ok(Self)
    }
}

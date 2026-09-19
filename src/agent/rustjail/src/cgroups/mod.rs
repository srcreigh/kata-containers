// Copyright (c) 2019,2020 Ant Financial
//
// SPDX-License-Identifier: Apache-2.0
//

use anyhow::Result;
use core::fmt::Debug;
use oci_spec::runtime::{LinuxDeviceCgroup, LinuxDeviceType, LinuxResources};
use protocols::agent::CgroupStats;

use crate::cgroups_rs as cgroups;
use cgroups::freezer::FreezerState;

pub mod fs;
#[cfg(test)]
pub mod mock;
pub mod notifier;

#[derive(Default, Debug)]
pub struct DevicesCgroupInfo {
    /// Indicate if the pod cgroup is initialized.
    inited: bool,
    /// Indicate if pod's devices cgroup is in whitelist mode. Returns true
    /// once one container requires `a *:* rwm` permission.
    allowed_all: bool,
}

pub trait Manager {
    fn apply(&self, pid: i32) -> Result<()>;
    fn get_pids(&self) -> Result<Vec<i32>>;
    fn get_stats(&self) -> Result<CgroupStats>;
    fn freeze(&self, state: FreezerState) -> Result<()>;
    fn destroy(&self) -> Result<()>;
    fn set(&self, resources: &LinuxResources) -> Result<()>;
    fn get_cgroup_path(&self) -> Result<String>;
    fn name(&self) -> &str;
}

impl Debug for dyn Manager + Send + Sync {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", self.name())
    }
}

/// Check if device cgroup is a rule for all devices from OCI spec.
///
/// The formats representing all devices between OCI spec and cgroups-rs
/// are different.
/// - OCI spec: major: Some(0), minor: Some(0), type: Some(A), access: Some("rwm");
/// - Cgroups-rs: major: -1, minor: -1, type: "a", access: "rwm";
/// - Linux: a *:* rwm
#[inline]
fn rule_for_all_devices(dev_cgroup: &LinuxDeviceCgroup) -> bool {
    let cgrp_access = dev_cgroup.access().clone().unwrap_or_default();
    let dev_type = dev_cgroup
        .typ()
        .as_ref()
        .map_or(LinuxDeviceType::default(), |x| *x);
    dev_cgroup.major().unwrap_or(0) == 0
        && dev_cgroup.minor().unwrap_or(0) == 0
        && dev_type == LinuxDeviceType::A
        && cgrp_access.contains('r')
        && cgrp_access.contains('w')
        && cgrp_access.contains('m')
}

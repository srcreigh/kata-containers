// Copyright (c) 2022 Alibaba Cloud
//
// SPDX-License-Identifier: Apache-2.0
//

use oci_spec::runtime as oci;
use std::convert::TryFrom;
use std::str::FromStr;

/// A set of CPU ids.
pub type CpuSet = crate::utils::u32_set::U32Set;

/// A set of NUMA memory nodes.
pub type NumaNodeSet = crate::utils::u32_set::U32Set;

/// Error code for CPU related operations.
#[derive(thiserror::Error, Debug)]
pub enum Error {
    /// Invalid CPU list.
    #[error("Invalid CPU list: {0}")]
    InvalidCpuSet(crate::Error),
    /// Invalid NUMA memory node list.
    #[error("Invalid NUMA memory node list: {0}")]
    InvalidNodeSet(crate::Error),
}

/// Assigned CPU resources for a Linux container.
/// Stores fractional vCPU allocation for more precise resource tracking.
#[derive(Clone, Default, Debug)]
pub struct LinuxContainerCpuResources {
    /// Calculated fractional vCPU allocation, e.g., 0.25 means 1/4 of a CPU.
    calculated_vcpu: Option<f64>,
}

impl LinuxContainerCpuResources {
    /// Get the number of vCPUs assigned to the container as a fractional value.
    /// Returns `None` if unconstrained (no limit).
    pub fn get_vcpus(&self) -> Option<f64> {
        self.calculated_vcpu
    }
}

impl TryFrom<&oci::LinuxCpu> for LinuxContainerCpuResources {
    type Error = Error;

    // Unhandled fields: realtime_runtime, realtime_period, mems
    fn try_from(value: &oci::LinuxCpu) -> Result<Self, Self::Error> {
        let period = value.period().unwrap_or(0);
        let quota = value.quota().unwrap_or(-1);
        let value_cpus = value.cpus().as_deref().unwrap_or("");
        let cpuset = CpuSet::from_str(value_cpus).map_err(Error::InvalidCpuSet)?;
        let value_mems = value.mems().as_deref().unwrap_or("");
        NumaNodeSet::from_str(value_mems).map_err(Error::InvalidNodeSet)?;

        // Calculate fractional vCPUs:
        // If quota >= 0 and period > 0, vCPUs = quota / period.
        // Otherwise, if cpuset is non-empty, derive from cpuset length.
        let vcpu_fraction = if quota >= 0 && period > 0 {
            Some(quota as f64 / period as f64)
        } else if !cpuset.is_empty() {
            Some(cpuset.len() as f64)
        } else {
            None
        };

        Ok(LinuxContainerCpuResources {
            calculated_vcpu: vcpu_fraction,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const EPSILON: f64 = 0.0001;

    #[test]
    fn test_linux_container_cpu_resources() {
        let resources = LinuxContainerCpuResources::default();

        assert!(resources.get_vcpus().is_none());

        let mut linux_cpu = oci::LinuxCpu::default();
        linux_cpu.set_shares(Some(2048));
        linux_cpu.set_quota(Some(1001));
        linux_cpu.set_period(Some(100));
        linux_cpu.set_cpus(Some("1,2,3".to_string()));
        linux_cpu.set_mems(Some("1".to_string()));

        let resources = LinuxContainerCpuResources::try_from(&linux_cpu).unwrap();

        // Expected fractional vCPUs = quota / period
        let expected_vcpus = 1001.0 / 100.0;
        assert!(
            (resources.get_vcpus().unwrap() - expected_vcpus).abs() < EPSILON,
            "got {}, expect {}",
            resources.get_vcpus().unwrap(),
            expected_vcpus
        );

        // Test cpuset-only path (no quota)
        let mut linux_cpu = oci::LinuxCpu::default();
        linux_cpu.set_shares(Some(2048));
        linux_cpu.set_cpus(Some("1".to_string()));
        linux_cpu.set_mems(Some("1-2".to_string()));

        let resources = LinuxContainerCpuResources::try_from(&linux_cpu).unwrap();
        assert!(
            (resources.get_vcpus().unwrap() - 1.0).abs() < EPSILON,
            "cpuset size vCPU mismatch"
        );
    }

    #[test]
    fn invalid_cpu_and_numa_sets_fail() {
        for (cpus, mems) in [("invalid", ""), ("", "invalid")] {
            let mut cpu = oci::LinuxCpu::default();
            cpu.set_cpus(Some(cpus.into()));
            cpu.set_mems(Some(mems.into()));
            assert!(LinuxContainerCpuResources::try_from(&cpu).is_err());
        }
    }
}

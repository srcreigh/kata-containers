// Copyright (c) 2019-2021 Alibaba Cloud
// Copyright (c) 2019 Ant Group
//
// SPDX-License-Identifier: Apache-2.0
//

use std::collections::HashMap;
use std::io::{self, Result};
use std::result::{self};

use serde::Deserialize;

use crate::config::hypervisor::get_hypervisor_plugin;

use crate::config::TomlConfig;
use crate::sl;

use self::cri_containerd::{SANDBOX_CPU_PERIOD_KEY, SANDBOX_CPU_QUOTA_KEY, SANDBOX_MEM_KEY};

/// CRI-containerd specific annotations.
pub mod cri_containerd;

/// CRI-O specific annotations.
pub mod crio;

/// Dockershim specific annotations.
pub mod dockershim;

/// Prefix for Kata configuration annotations
pub const KATA_ANNO_CFG_PREFIX: &str = "io.katacontainers.config.";
/// The annotation key to fetch the OCI configuration file path.
pub const BUNDLE_PATH_KEY: &str = "io.katacontainers.pkg.oci.bundle_path";
/// The annotation key to fetch container type.
pub const CONTAINER_TYPE_KEY: &str = "io.katacontainers.pkg.oci.container_type";

/// A sandbox annotation to enable tracing for the agent.
pub const KATA_ANNO_CFG_AGENT_TRACE: &str = "io.katacontainers.config.agent.enable_tracing";
/// Prefix for Hypervisor configurations.
pub const KATA_ANNO_CFG_HYPERVISOR_PREFIX: &str = "io.katacontainers.config.hypervisor.";
/// A sandbox annotation to specify the number of independent IO threads.
/// Used for virtio-blk-pci devices during hotplug.
pub const KATA_ANNO_CFG_HYPERVISOR_INDEP_IO_THREADS: &str =
    "io.katacontainers.config.hypervisor.indep_iothreads";
/// SHA512 is the SHA-512 (64) hash algorithm
pub const SHA512: &str = "sha512";

/// A sandbox annotation for passing a per container path pointing at the kernel needed to boot
/// the container VM.
pub const KATA_ANNO_CFG_HYPERVISOR_KERNEL_PATH: &str = "io.katacontainers.config.hypervisor.kernel";

/// A sandbox annotation for passing the default vCPUs assigned for a VM by the hypervisor.
pub const KATA_ANNO_CFG_HYPERVISOR_DEFAULT_VCPUS: &str =
    "io.katacontainers.config.hypervisor.default_vcpus";

/// A sandbox annotation for the memory assigned for a VM by the hypervisor.
pub const KATA_ANNO_CFG_HYPERVISOR_DEFAULT_MEMORY: &str =
    "io.katacontainers.config.hypervisor.default_memory";

/// The initdata annotation passed in when CVM launchs
pub const KATA_ANNO_CFG_HYPERVISOR_INIT_DATA: &str =
    "io.katacontainers.config.hypervisor.cc_init_data";

/// A helper structure to query configuration information by check annotations.
#[derive(Debug, Default, Deserialize)]
pub struct Annotation {
    annotations: HashMap<String, String>,
}

impl From<HashMap<String, String>> for Annotation {
    fn from(annotations: HashMap<String, String>) -> Self {
        Annotation { annotations }
    }
}

impl Annotation {
    /// Create a new instance of [`Annotation`].
    pub fn new(annotations: HashMap<String, String>) -> Annotation {
        Annotation { annotations }
    }

    /// Get the value of annotation with `key`
    pub fn get_value<T>(
        &self,
        key: &str,
    ) -> result::Result<Option<T>, <T as std::str::FromStr>::Err>
    where
        T: std::str::FromStr,
    {
        if let Some(value) = self.get(key) {
            return value.parse::<T>().map(Some);
        }
        Ok(None)
    }

    /// Get the value of annotation with `key` as string.
    pub fn get(&self, key: &str) -> Option<String> {
        self.annotations.get(key).map(|v| String::from(v.trim()))
    }
}

// Miscellaneous annotations.
impl Annotation {
    /// Get the annotation of cpu quota for sandbox
    pub fn get_sandbox_cpu_quota(&self) -> i64 {
        let value = self
            .get_value::<i64>(SANDBOX_CPU_QUOTA_KEY)
            .unwrap_or(Some(0));
        value.unwrap_or(0)
    }

    /// Get the annotation of cpu period for sandbox
    pub fn get_sandbox_cpu_period(&self) -> u64 {
        let value = self
            .get_value::<u64>(SANDBOX_CPU_PERIOD_KEY)
            .unwrap_or(Some(0));
        value.unwrap_or(0)
    }

    /// Get the annotation of memory for sandbox
    pub fn get_sandbox_mem(&self) -> i64 {
        let value = self.get_value::<i64>(SANDBOX_MEM_KEY).unwrap_or(Some(0));
        value.unwrap_or(0)
    }

    /// Get the pod's summed container resources as reported by CRI-O.
    ///
    /// CRI-O states them as one JSON annotation rather than the per-resource
    /// keys containerd uses, so a caller after sandbox sizing has to ask for
    /// them separately. Returns `None` when the annotation is absent or does
    /// not parse.
    pub fn get_crio_pod_linux_resources(&self) -> Option<crio::PodLinuxResources> {
        let value = self
            .get(crio::POD_LINUX_RESOURCES_KEY)
            .or_else(|| self.get(crio::POD_LINUX_RESOURCES_KEY_DEPRECATED))?;

        match serde_json::from_str::<crio::PodLinuxResources>(&value) {
            Ok(resources) => Some(resources),
            Err(e) => {
                warn!(
                    sl!(),
                    "sandbox-sizing: failed to parse CRI-O pod resources: {}", e
                );
                None
            }
        }
    }
}

/// Reject configuration annotations outside the Kubernetes/Firecracker contract.
pub fn validate_config_annotations(annotations: &HashMap<String, String>) -> Result<()> {
    for key in annotations
        .keys()
        .filter(|key| key.starts_with(KATA_ANNO_CFG_PREFIX))
    {
        if !matches!(
            key.as_str(),
            KATA_ANNO_CFG_HYPERVISOR_DEFAULT_VCPUS
                | KATA_ANNO_CFG_HYPERVISOR_DEFAULT_MEMORY
                | KATA_ANNO_CFG_AGENT_TRACE
                | "io.katacontainers.config.runtime.enable_tracing"
        ) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("kata-fc-minimal: unsupported configuration annotation {key}"),
            ));
        }
    }
    Ok(())
}

impl Annotation {
    /// Apply the supported sizing and tracing annotations.
    pub fn update_config_by_annotation(&self, config: &mut TomlConfig) -> Result<()> {
        validate_config_annotations(&self.annotations)?;
        for (name, value) in [
            ("Runtime", &config.runtime.name),
            ("Hypervisor", &config.runtime.hypervisor_name),
            ("Agent", &config.runtime.agent_name),
        ] {
            if value.is_empty() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{name} name is missing in the configuration"),
                ));
            }
        }
        let hypervisor_name = &config.runtime.hypervisor_name;
        let agent_name = &config.runtime.agent_name;
        let hv = config.hypervisor.get_mut(hypervisor_name).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Invalid hypervisor name {hypervisor_name}"),
            )
        })?;
        let ag = config.agent.get_mut(agent_name).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Invalid agent name {agent_name}"),
            )
        })?;
        for (key, value) in &self.annotations {
            match key.as_str() {
                KATA_ANNO_CFG_HYPERVISOR_DEFAULT_VCPUS
                    if hv.security_info.is_annotation_enabled(key) =>
                {
                    let cpus = self
                        .get_value::<f32>(key)
                        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "parse i32 error"))?
                        .unwrap_or_default();
                    let max = get_hypervisor_plugin(hypervisor_name)
                        .unwrap()
                        .get_max_cpus();
                    if cpus > max as f32 {
                        return Err(io::Error::new(io::ErrorKind::InvalidData,
                            format!("Vcpus specified in annotation {cpus} is more than maximum limitation {max}")));
                    }
                    hv.cpu_info.default_vcpus = cpus;
                }
                KATA_ANNO_CFG_HYPERVISOR_DEFAULT_MEMORY
                    if hv.security_info.is_annotation_enabled(key) =>
                {
                    if let Some(memory) = convert_to_megabytes(value)? {
                        let min = get_hypervisor_plugin(hypervisor_name)
                            .unwrap()
                            .get_min_memory();
                        if memory < min {
                            return Err(io::Error::new(io::ErrorKind::InvalidData,
                                format!("memory specified in annotation {memory} is less than minimum limitation {min}")));
                        }
                        hv.memory_info.default_memory = memory;
                    }
                }
                KATA_ANNO_CFG_AGENT_TRACE => {
                    ag.enable_tracing = self
                        .get_value::<bool>(key)
                        .map_err(|_| {
                            io::Error::new(io::ErrorKind::InvalidData, "parse bool error")
                        })?
                        .unwrap_or_default();
                }
                "io.katacontainers.config.runtime.enable_tracing" => {
                    config.runtime.enable_tracing = self
                        .get_value::<bool>(key)
                        .map_err(|_| {
                            io::Error::new(io::ErrorKind::InvalidData, "parse bool error")
                        })?
                        .unwrap_or_default();
                }
                _ => {}
            }
        }
        config.adjust_config()
    }
}

fn convert_to_megabytes(mem_size_str: &str) -> Result<Option<u32>> {
    match byte_unit::Byte::parse_str(mem_size_str, true) {
        Ok(mut mem_size) => {
            let no_suffix_given = mem_size_str
                .trim()
                .chars()
                .all(|c: char| c.is_ascii_digit());
            if no_suffix_given {
                // NOTE the error is apparently unreachable at the moment:
                // Byte::from_u64_with_unit() doesn't fail unless its argument
                // is too big, however that same too big arg will fail to
                // Byte::parse_str() in the first place.  (Obviously we still
                // need to handle it anyway.)
                mem_size =
                    byte_unit::Byte::from_u64_with_unit(mem_size.as_u64(), byte_unit::Unit::MiB)
                        .ok_or(io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!("failed to convert {} to MiB", mem_size.as_u64()),
                        ))?;
            }
            let memory_size = mem_size.get_adjusted_unit(byte_unit::Unit::MiB).get_value() as u32;
            Ok(Some(memory_size))
        }
        Err(error) => {
            error!(
                sl!(),
                "failed to parse byte from string {} error {:?}", mem_size_str, error
            );
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_memory_no_unit() {
        let result = convert_to_megabytes("2048");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Some(2048));
    }

    #[test]
    fn parse_memory_with_units() {
        let result = convert_to_megabytes("2 GiB");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Some(2048));
    }

    #[test]
    fn parse_memory_parse_error() {
        let result = convert_to_megabytes("2048r");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), None);
    }
}

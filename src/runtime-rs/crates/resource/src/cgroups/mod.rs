// Copyright (c) 2019-2022 Alibaba Cloud
// Copyright (c) 2019-2022 Ant Group
//
// SPDX-License-Identifier: Apache-2.0
//

pub mod cgroup_persist;
mod resource;
pub use resource::CgroupsResource;
mod resource_inner;

use anyhow::{anyhow, ensure, Result};
use kata_sys_util::spec::load_oci_spec;
use kata_types::config::TomlConfig;

use crate::cgroups::cgroup_persist::CgroupState;

const SANDBOXED_CGROUP_PATH: &str = "kata_sandboxed_pod";

pub struct CgroupArgs {
    pub sid: String,
    pub config: TomlConfig,
}

pub struct CgroupConfig {
    pub path: String,
}

impl CgroupConfig {
    fn new(sid: &str, toml_config: &TomlConfig) -> Result<Self> {
        validate_mode(
            toml_config.runtime.sandbox_cgroup_only,
            toml_config.runtime.enable_vcpus_pinning,
        )?;
        let path = if let Ok(spec) = load_oci_spec() {
            spec.linux()
                .clone()
                .and_then(|linux| linux.cgroups_path().clone())
                .map(|path| {
                    // The trim of '/' is important, because cgroup_path is a relative path.
                    path.display()
                        .to_string()
                        .trim_start_matches('/')
                        .to_string()
                })
                .unwrap_or_default()
        } else {
            format!("{SANDBOXED_CGROUP_PATH}/{sid}")
        };

        Ok(Self { path })
    }

    fn restore(state: &CgroupState) -> Result<Self> {
        // Check saved modes before constructing any manager or changing cgroups.
        validate_mode(state.sandbox_cgroup_only, state.enable_vcpus_pinning)?;
        let path = state
            .path
            .as_ref()
            .ok_or_else(|| anyhow!("cgroup path is missing in state"))?;
        Ok(Self { path: path.clone() })
    }
}

fn validate_mode(sandbox_cgroup_only: bool, enable_vcpus_pinning: bool) -> Result<()> {
    ensure!(
        sandbox_cgroup_only,
        "kata-fc requires sandbox_cgroup_only=true"
    );
    ensure!(
        !enable_vcpus_pinning,
        "kata-fc does not support vCPU pinning"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_legacy_state_ignores_unused_overhead_path() {
        let state = serde_json::from_value(serde_json::json!({
            "path": "kubepods.slice:cri-containerd:pod",
            "overhead_path": "kata-overhead.slice:runtime-rs:pod",
            "sandbox_cgroup_only": true,
            "enable_vcpus_pinning": false
        }))
        .unwrap();
        assert_eq!(
            CgroupConfig::restore(&state).unwrap().path,
            "kubepods.slice:cri-containerd:pod"
        );
    }

    #[test]
    fn unsupported_saved_modes_are_rejected_before_path_or_manager_use() {
        for (sandbox_only, pinning, error) in [
            (false, false, "sandbox_cgroup_only"),
            (true, true, "vCPU pinning"),
        ] {
            // An absent path demonstrates that mode rejection occurs before even
            // resolving a manager target, so these states cannot mutate cgroups.
            let state = CgroupState {
                path: None,
                sandbox_cgroup_only: sandbox_only,
                enable_vcpus_pinning: pinning,
            };
            assert!(CgroupConfig::restore(&state)
                .err()
                .unwrap()
                .to_string()
                .contains(error));
        }
        // Old valid states always serialized both booleans; missing modes fail closed.
        let incomplete: CgroupState = serde_json::from_str(r#"{"path":"pod"}"#).unwrap();
        assert!(CgroupConfig::restore(&incomplete).is_err());
    }

    #[test]
    fn unsupported_new_modes_are_rejected_before_loading_oci() {
        let mut config = TomlConfig::default();
        config.runtime.sandbox_cgroup_only = false;
        assert!(CgroupConfig::new("pod", &config)
            .err()
            .unwrap()
            .to_string()
            .contains("sandbox_cgroup_only"));
        config.runtime.sandbox_cgroup_only = true;
        config.runtime.enable_vcpus_pinning = true;
        assert!(CgroupConfig::new("pod", &config)
            .err()
            .unwrap()
            .to_string()
            .contains("vCPU pinning"));
    }
}

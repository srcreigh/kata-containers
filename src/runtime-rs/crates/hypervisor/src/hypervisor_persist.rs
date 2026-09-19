// Copyright (c) 2019-2022 Alibaba Cloud
// Copyright (c) 2019-2022 Ant Group
//
// SPDX-License-Identifier: Apache-2.0
//

use crate::HypervisorConfig;
use serde::{Deserialize, Serialize};
#[derive(Serialize, Deserialize, Default, Clone, Debug)]
pub struct HypervisorState {
    // Type of hypervisor.
    pub hypervisor_type: String,
    /// sandbox id
    pub id: String,
    /// vm path
    pub vm_path: String,
    /// jailed flag
    pub jailed: bool,
    /// chroot base for the jailer
    pub jailer_root: String,
    /// netns
    pub netns: Option<String>,
    /// hypervisor config
    pub config: HypervisorConfig,
    /// hypervisor run dir
    pub run_dir: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_unused_backend_fields_do_not_break_restore() {
        let mut state = HypervisorState::default();
        state.jailed = true;
        let mut value = serde_json::to_value(&state).unwrap();
        let object = value.as_object_mut().unwrap();
        object.insert("cached_block_devices".into(), serde_json::json!(["unused"]));
        object.insert("virtiofs_daemon_pid".into(), serde_json::json!(123));
        object.insert("passfd_listener_port".into(), serde_json::json!(10240));
        object.insert(
            "guest_protection_to_use".into(),
            serde_json::json!("NoProtection"),
        );
        let decoded: HypervisorState = serde_json::from_value(value).unwrap();
        assert!(decoded.jailed);
    }
}

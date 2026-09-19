// Copyright (c) 2019-2022 Alibaba Cloud
// Copyright (c) 2019-2025 Ant Group
//
// SPDX-License-Identifier: Apache-2.0
//

use std::sync::Arc;

use anyhow::{Context, Result};
use async_trait::async_trait;
use kata_types::config::TomlConfig;
use oci_spec::runtime::LinuxResources;
use persist::sandbox_persist::Persist;
use tokio::sync::RwLock;

use crate::cgroups::cgroup_persist::CgroupState;
use crate::cgroups::resource_inner::CgroupsResourceInner;
use crate::cgroups::{CgroupArgs, CgroupConfig};
use crate::ResourceUpdateOp;

/// CgroupsResource places the runtime and Firecracker in the sandbox cgroup,
/// preserving the orchestrator's whole-sandbox CPU and memory accounting.
pub struct CgroupsResource {
    cgroup_config: CgroupConfig,
    inner: Arc<RwLock<CgroupsResourceInner>>,
}

impl CgroupsResource {
    pub fn new(sid: &str, toml_config: &TomlConfig) -> Result<Self> {
        let cgroup_config = CgroupConfig::new(sid, toml_config)?;
        let inner = CgroupsResourceInner::new(&cgroup_config)?;
        let inner = Arc::new(RwLock::new(inner));

        Ok(Self {
            cgroup_config,
            inner,
        })
    }
}

impl CgroupsResource {
    pub async fn delete(&self) -> Result<()> {
        let mut inner = self.inner.write().await;
        inner.delete().await
    }

    pub async fn update(
        &self,
        cid: &str,
        resources: Option<&LinuxResources>,
        op: ResourceUpdateOp,
    ) -> Result<()> {
        let mut inner = self.inner.write().await;
        inner.update(cid, resources, op).await
    }

    pub async fn setup_after_start_vm(&self) -> Result<()> {
        let mut inner = self.inner.write().await;
        inner.setup_after_start_vm().await
    }
}

#[async_trait]
impl Persist for CgroupsResource {
    type State = CgroupState;
    type ConstructorArgs = CgroupArgs;
    /// Save a state of the component.
    async fn save(&self) -> Result<Self::State> {
        Ok(CgroupState {
            path: Some(self.cgroup_config.path.clone()),
            sandbox_cgroup_only: true,
            enable_vcpus_pinning: false,
        })
    }

    /// Restore a component from a specified state.
    async fn restore(
        _cgroup_args: Self::ConstructorArgs,
        cgroup_state: Self::State,
    ) -> Result<Self> {
        let cgroup_config = CgroupConfig::restore(&cgroup_state)?;
        let inner = CgroupsResourceInner::restore(&cgroup_config)
            .context("restore cgroups resource inner")?;
        let inner = Arc::new(RwLock::new(inner));

        Ok(Self {
            cgroup_config,
            inner,
        })
    }
}

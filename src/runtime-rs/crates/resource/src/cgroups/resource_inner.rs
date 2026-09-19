// Copyright (c) 2019-2022 Alibaba Cloud
// Copyright (c) 2019-2025 Ant Group
//
// SPDX-License-Identifier: Apache-2.0
//

use std::collections::{HashMap, HashSet};
use std::error::Error as _;
use std::process;

use anyhow::{Context, Result};
use cgroups::manager::is_systemd_cgroup;
use cgroups::{CgroupPid, FsManager, Manager, SystemdManager};
use oci_spec::runtime::{LinuxCpu, LinuxCpuBuilder, LinuxResources, LinuxResourcesBuilder};

use crate::cgroups::CgroupConfig;
use crate::ResourceUpdateOp;

pub type CgroupManager = Box<dyn Manager>;

pub(crate) struct CgroupsResourceInner {
    /// Container resources, key is container id, and value is resources.
    resources: HashMap<String, LinuxResources>,
    sandbox_cgroup: CgroupManager,
}

impl CgroupsResourceInner {
    fn is_already_exists_error(err: &cgroups::manager::Error) -> bool {
        let mut source = err.source();

        while let Some(inner_err) = source {
            if let Some(io_err) = inner_err.downcast_ref::<std::io::Error>() {
                if io_err.kind() == std::io::ErrorKind::AlreadyExists {
                    return true;
                }
            }

            source = inner_err.source();
        }

        false
    }

    fn add_proc_with_existing_retry(
        cgroup: &mut CgroupManager,
        pid: CgroupPid,
        context: &str,
    ) -> Result<()> {
        match cgroup.add_proc(pid) {
            Ok(_) => Ok(()),
            Err(err) if Self::is_already_exists_error(&err) => cgroup
                .add_proc(pid)
                .with_context(|| format!("{context} (retry after pre-existing cgroup)")),
            Err(err) => Err(err).context(context.to_string()),
        }
    }

    /// Synchronously write the runtime's own pid into the sandbox cgroup's
    /// `cgroup.procs` (cgroup v2 only; a no-op otherwise).
    ///
    /// `add_proc` already places the runtime, but on the systemd driver that
    /// is an asynchronous call: the runtime may fork the
    /// VMM before systemd has moved it, so those children inherit its original
    /// cgroup (e.g. `system.slice/containerd.service`). Under cgroup v2
    /// first-touch accounting the guest RAM is then charged there, not to the
    /// pod. A direct write is synchronous, so children forked afterwards
    /// inherit the sandbox cgroup.
    fn place_runtime_in_sandbox_cgroup_v2_sync(cgroup: &CgroupManager) -> Result<()> {
        if !cgroup.v2() {
            return Ok(());
        }
        let dir = cgroup
            .cgroup_path(None)
            .context("resolve sandbox cgroup path for runtime placement")?;
        let procs_path = format!("{}/cgroup.procs", dir.trim_end_matches('/'));
        let pid = process::id();
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .open(&procs_path)
            .with_context(|| format!("open sandbox cgroup.procs {procs_path}"))?;
        std::io::Write::write_all(&mut file, format!("{pid}\n").as_bytes())
            .with_context(|| format!("move runtime pid {pid} into sandbox cgroup {procs_path}"))?;
        info!(
            sl!(),
            "synchronously placed runtime (pid {}) into sandbox cgroup: {}", pid, procs_path
        );
        Ok(())
    }

    /// Build the sandbox manager; host systemd and cgroupfs remain supported.
    fn new_cgroup_manager(config: &CgroupConfig) -> Result<CgroupManager> {
        let use_systemd = is_systemd_cgroup(&config.path);
        let sandbox_cgroup = if use_systemd {
            let mut manager = SystemdManager::new(&config.path).context("new systemd manager")?;
            // Set SIGTERM timeout to 5mins, so that the runtime has up to
            // 5mins to do graceful shutdown. Exceeding this timeout, the
            // systemd will forcibly kill the runtime by sending SIGKILL.
            manager.set_term_timeout(300).context("set term timeout")?;
            Box::new(manager) as Box<dyn Manager>
        } else {
            let manager = FsManager::new(&config.path).context("new fs manager")?;
            Box::new(manager) as Box<dyn Manager>
        };

        Ok(sandbox_cgroup)
    }

    /// Create a new `CgroupsResourceInner` instance.
    pub(crate) fn new(config: &CgroupConfig) -> Result<Self> {
        let mut sandbox_cgroup = Self::new_cgroup_manager(config).context("create new cgroup")?;
        let pid = CgroupPid::from(process::id() as u64);
        Self::add_proc_with_existing_retry(
            &mut sandbox_cgroup,
            pid,
            "add runtime to sandbox cgroup",
        )?;
        // The systemd call is asynchronous: enter the sandbox before forking children.
        Self::place_runtime_in_sandbox_cgroup_v2_sync(&sandbox_cgroup)
            .context("synchronously place runtime in sandbox cgroup")?;

        Ok(Self {
            resources: HashMap::new(),
            sandbox_cgroup,
        })
    }

    pub(crate) fn restore(config: &CgroupConfig) -> Result<Self> {
        let sandbox_cgroup = Self::new_cgroup_manager(config).context("restore cgroup")?;
        Ok(Self {
            resources: HashMap::new(),
            sandbox_cgroup,
        })
    }
}

impl CgroupsResourceInner {
    /// Add cpuset resources of all containers to the sandbox cgroup.
    fn collect_resources(&self) -> Result<LinuxResources> {
        let mut cpu_cpus = HashSet::new();
        let mut cpu_mems = HashSet::new();

        for res in self.resources.values() {
            if let Some(cpu) = res.cpu() {
                if let Some(cpus) = cpu.cpus() {
                    cpu_cpus.insert(cpus.to_string());
                }
                if let Some(mems) = cpu.mems() {
                    cpu_mems.insert(mems.to_string());
                }
            }
        }

        let mut resources_builder = LinuxResourcesBuilder::default();

        let mut cpu_builder = LinuxCpuBuilder::default();
        if !cpu_cpus.is_empty() {
            cpu_builder = cpu_builder.cpus(cpu_cpus.into_iter().collect::<Vec<_>>().join(","));
        }
        if !cpu_mems.is_empty() {
            cpu_builder = cpu_builder.mems(cpu_mems.into_iter().collect::<Vec<_>>().join(","));
        }
        let cpu = cpu_builder.build().context("build linux cpu")?;
        if cpu != LinuxCpu::default() {
            resources_builder = resources_builder.cpu(cpu);
        }

        let resources = resources_builder.build().context("build linux resources")?;

        Ok(resources)
    }

    fn update_sandbox_cgroups(&mut self) -> Result<()> {
        let sandbox_resources = self.collect_resources().context("collect resources")?;
        self.sandbox_cgroup.set(&sandbox_resources).context("set")?;
        Ok(())
    }
}

impl CgroupsResourceInner {
    pub(crate) async fn delete(&mut self) -> Result<()> {
        self.sandbox_cgroup
            .destroy()
            .context("destroy sandbox cgroup")?;

        Ok(())
    }

    pub(crate) async fn update(
        &mut self,
        cid: &str,
        resources: Option<&LinuxResources>,
        op: ResourceUpdateOp,
    ) -> Result<()> {
        let old = match op {
            ResourceUpdateOp::Add | ResourceUpdateOp::Update => {
                let resources = resources.ok_or_else(|| {
                    anyhow::anyhow!("resources should not be empty for Add or Update operation")
                })?;
                let new = new_cpuset_resources(resources).context("new cpuset resources")?;
                let old = self.resources.insert(cid.to_string(), new.clone());
                // If the new resources are the same as the old ones, we
                // can skip the update.
                if let Some(old) = old.as_ref() {
                    if old == &new {
                        return Ok(());
                    }
                }
                old
            }
            ResourceUpdateOp::Del => self.resources.remove(cid),
        };

        let ret = self
            .update_sandbox_cgroups()
            .context("update sandbox cgroups");

        // Rollback if the update fails
        if ret.is_err() {
            match op {
                ResourceUpdateOp::Add => {
                    self.resources.remove(cid);
                }
                ResourceUpdateOp::Update | ResourceUpdateOp::Del => {
                    if let Some(old) = old {
                        self.resources.insert(cid.to_string(), old);
                    }
                }
            }
        }

        ret
    }

    pub(crate) async fn setup_after_start_vm(&mut self) -> Result<()> {
        self.update_sandbox_cgroups()
            .context("update sandbox cgroups after start vm")
    }
}

/// Copy cpu.cpus and cpu.mems from the given resources to new resources.
fn new_cpuset_resources(resources: &LinuxResources) -> Result<LinuxResources> {
    let cpu = resources.cpu();
    let cpus = cpu.as_ref().and_then(|c| c.cpus().clone());
    let mems = cpu.as_ref().and_then(|c| c.mems().clone());

    let mut builder = LinuxCpuBuilder::default();
    if let Some(cpus) = cpus {
        builder = builder.cpus(cpus);
    }
    if let Some(mems) = mems {
        builder = builder.mems(mems);
    }
    let linux_cpu = builder.build().context("build linux cpu")?;

    let builder = LinuxResourcesBuilder::default().cpu(linux_cpu);
    let resources = builder.build().context("build linux resources")?;

    Ok(resources)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kata_types::cpu::CpuSet;
    use std::str::FromStr;

    #[test]
    fn whole_sandbox_cpuset_combines_containers_and_preserves_memory_nodes() {
        // FsManager construction resolves paths but does not place a process or set limits.
        let mut inner = CgroupsResourceInner {
            resources: HashMap::new(),
            sandbox_cgroup: Box::new(FsManager::new("test_sandbox_cpuset").unwrap()),
        };
        for (id, cpus, mems) in [("a", "0-2", "0"), ("b", "2,4", "1")] {
            let cpu = LinuxCpuBuilder::default()
                .cpus(cpus)
                .mems(mems)
                .shares(1024u64)
                .build()
                .unwrap();
            let resources = LinuxResourcesBuilder::default().cpu(cpu).build().unwrap();
            inner
                .resources
                .insert(id.to_string(), new_cpuset_resources(&resources).unwrap());
        }
        let resources = inner.collect_resources().unwrap();
        let cpu = resources.cpu().as_ref().unwrap();
        assert_eq!(
            CpuSet::from_str(cpu.cpus().as_ref().unwrap()).unwrap(),
            CpuSet::from_str("0-2,4").unwrap()
        );
        assert_eq!(
            CpuSet::from_str(cpu.mems().as_ref().unwrap()).unwrap(),
            CpuSet::from_str("0,1").unwrap()
        );
        assert!(cpu.shares().is_none());
        inner.resources.clear();
        assert!(inner.collect_resources().unwrap().cpu().is_none());
    }
}

// Copyright (c) 2019-2022 Alibaba Cloud
// Copyright (c) 2019-2022 Ant Group
//
// SPDX-License-Identifier: Apache-2.0
//

use std::{collections::HashMap, sync::Arc, thread};

use agent::{types::Device, ARPNeighbor, Agent, Storage};
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use hypervisor::{
    device::{
        device_manager::{do_handle_device, get_block_device_info, DeviceManager},
        util::{get_host_path, DEVICE_TYPE_BLOCK, DEVICE_TYPE_CHAR},
        DeviceConfig, DeviceType,
    },
    BlockConfigModern, Hypervisor,
};
use kata_types::config::TomlConfig;
use kata_types::mount::{kata_guest_sandbox_dir, Mount, KATA_EPHEMERAL_VOLUME_TYPE, SHM_DIR};
use libc::NUD_PERMANENT;
use oci::{Linux, LinuxResources};
use oci_spec::runtime::{self as oci, LinuxDeviceType};
use persist::sandbox_persist::Persist;
use tokio::{runtime, sync::RwLock};

use crate::{
    cgroups::{CgroupArgs, CgroupsResource},
    manager::ManagerArgs,
    network::{self, Network, NetworkConfig},
    resource_persist::ResourceState,
    rootfs::{RootFsResource, Rootfs},
    volume::{utils::is_block_device_readonly, Volume, VolumeResource},
    ResourceConfig, ResourceUpdateOp,
};

pub(crate) struct ResourceManagerInner {
    sid: String,
    toml_config: Arc<TomlConfig>,
    agent: Arc<dyn Agent>,
    hypervisor: Arc<dyn Hypervisor>,
    device_manager: Arc<RwLock<DeviceManager>>,
    network: Option<Arc<dyn Network>>,

    pub rootfs_resource: RootFsResource,
    pub volume_resource: VolumeResource,
    pub cgroups_resource: CgroupsResource,
}

// Namespace changes stay on a dedicated OS thread. Await its blocking join so
// Tokio can drive tasks that the namespace thread depends on, including an
// existing Firecracker API connection owned by the outer runtime.
async fn run_network_thread<T: Send + 'static>(
    work: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    tokio::task::spawn_blocking(move || {
        thread::spawn(work)
            .join()
            .map_err(|e| anyhow!("{:?}", e))
            .context("Couldn't join on the associated thread")?
    })
    .await
    .context("Couldn't await the network thread join")?
}

impl ResourceManagerInner {
    pub(crate) async fn new(
        sid: &str,
        agent: Arc<dyn Agent>,
        hypervisor: Arc<dyn Hypervisor>,
        toml_config: Arc<TomlConfig>,
    ) -> Result<Self> {
        // create device manager
        let dev_manager = DeviceManager::new(hypervisor.clone())
            .await
            .context("failed to create device manager")?;
        let device_manager = Arc::new(RwLock::new(dev_manager));

        let cgroups_resource = CgroupsResource::new(sid, &toml_config)?;
        Ok(Self {
            sid: sid.to_string(),
            toml_config,
            agent,
            hypervisor,
            device_manager,
            network: None,
            rootfs_resource: RootFsResource::new(),
            volume_resource: VolumeResource::new(),
            cgroups_resource,
        })
    }

    pub fn config(&self) -> Arc<TomlConfig> {
        self.toml_config.clone()
    }

    pub fn get_device_manager(&self) -> Arc<RwLock<DeviceManager>> {
        self.device_manager.clone()
    }

    pub async fn prepare_before_start_vm(
        &mut self,
        device_configs: Vec<ResourceConfig>,
    ) -> Result<()> {
        for dc in device_configs {
            match dc {
                ResourceConfig::Network(c) => {
                    self.handle_network(c)
                        .await
                        .context("failed to handle network")?;
                }
                ResourceConfig::VmRootfs(r) => {
                    do_handle_device(&self.device_manager, &DeviceConfig::BlockCfgModern(r))
                        .await
                        .context("do handle device failed.")?;
                }
            };
        }

        Ok(())
    }

    pub async fn handle_network(&mut self, network_config: NetworkConfig) -> Result<()> {
        // 1. When using Rust asynchronous programming, we use .await to
        //    allow other task to run instead of waiting for the completion of the current task.
        // 2. Also, when handling the pod network, we need to set the shim threads
        //    into the network namespace to perform those operations.
        // However, as the increase of the I/O intensive tasks, two issues could be caused by the two points above:
        // a. When the future is blocked, the current thread (which is in the pod netns)
        //    might be take over by other tasks. After the future is finished, the thread take over
        //    the current task might not be in the pod netns. But the current task still need to run in pod netns
        // b. When finish setting up the network, the current thread will be set back to the host namespace.
        //    In Rust Async, if the current thread is taken over by other task, the netns is dropped on another thread,
        //    but it is not in netns. So, the previous thread would still remain in the pod netns.
        // The solution is to block the future on the current thread, it is enabled by spawn an os thread, create a
        // tokio runtime, and block the task on it.
        let device_manager = self.device_manager.clone();
        let network = run_network_thread(move || -> Result<Arc<dyn Network>> {
            let rt = runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            let d = rt
                .block_on(network::new(&network_config, device_manager))
                .context("new network")?;
            rt.block_on(d.setup()).context("setup network")?;
            Ok(d)
        })
        .await
        .context("failed to set up network")?;
        self.network = Some(network);
        Ok(())
    }

    async fn handle_interfaces(&self, network: &dyn Network) -> Result<()> {
        for i in network.interfaces().await.context("get interfaces")? {
            info!(sl!(), "update interface {:?}", i);

            // After hotplugging a network device, the guest kernel needs time
            // to probe it before the interface appears.  This is especially
            // pronounced on s390x (CCW bus) but can also happen on x86 in
            // slower CI environments.  Retry a few times.
            let mut last_error = None;
            for attempt in 0..10u32 {
                match self
                    .agent
                    .update_interface(agent::UpdateInterfaceRequest {
                        interface: Some(i.clone()),
                    })
                    .await
                {
                    core::result::Result::Ok(_) => {
                        last_error = None;
                        break;
                    }
                    core::result::Result::Err(e) => {
                        debug!(
                            sl!(),
                            "update_interface attempt {} failed, retrying: {:?}",
                            attempt + 1,
                            e
                        );
                        last_error = Some(e);
                        if attempt < 9 {
                            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                        }
                    }
                }
            }
            if let Some(err) = last_error {
                return Err(err).context("update interface");
            }
        }

        Ok(())
    }

    async fn handle_neighbours(&self, network: &dyn Network) -> Result<()> {
        let all_neighbors = network.neighs().await.context("neighs")?;

        // We add only static ARP entries
        let neighbors: Vec<ARPNeighbor> = all_neighbors
            .iter()
            .filter(|n| n.state == NUD_PERMANENT as i32)
            .cloned()
            .collect();
        if !neighbors.is_empty() {
            info!(sl!(), "update neighbors {:?}", neighbors);
            self.agent
                .add_arp_neighbors(agent::AddArpNeighborRequest {
                    neighbors: Some(agent::ARPNeighbors { neighbors }),
                })
                .await
                .context("update neighbors")?;
        }
        Ok(())
    }

    async fn handle_routes(&self, network: &dyn Network) -> Result<()> {
        let routes = network.routes().await.context("routes")?;
        if !routes.is_empty() {
            info!(sl!(), "update routes {:?}", routes);
            self.agent
                .update_routes(agent::UpdateRoutesRequest {
                    route: Some(agent::Routes { routes }),
                })
                .await
                .context("update routes")?;
        }
        Ok(())
    }

    pub async fn setup_after_start_vm(&mut self) -> Result<()> {
        self.cgroups_resource
            .setup_after_start_vm()
            .await
            .context("setup cgroups after start vm")?;

        if let Some(network) = self.network.as_ref() {
            self.apply_network_to_agent(network.as_ref()).await?;
        }

        Ok(())
    }

    pub async fn apply_network_to_agent(&self, network: &dyn Network) -> Result<()> {
        self.handle_interfaces(network)
            .await
            .context("handle interfaces")?;
        self.handle_neighbours(network)
            .await
            .context("handle neighbors")?;
        self.handle_routes(network).await.context("handle routes")?;
        Ok(())
    }

    pub async fn get_storage_for_sandbox(&self, shm_size: u64) -> Result<Vec<Storage>> {
        let mut storages = vec![];

        let shm_size_option = format!("size={shm_size}");
        let mount_point = format!("{}/{}", kata_guest_sandbox_dir(), SHM_DIR);

        let shm_storage = Storage {
            driver: KATA_EPHEMERAL_VOLUME_TYPE.to_string(),
            mount_point,
            source: "shm".to_string(),
            fs_type: "tmpfs".to_string(),
            options: vec![
                "noexec".to_string(),
                "nosuid".to_string(),
                "nodev".to_string(),
                "mode=1777".to_string(),
                shm_size_option,
            ],
            ..Default::default()
        };

        storages.push(shm_storage);

        Ok(storages)
    }

    pub async fn handler_rootfs(
        &self,
        cid: &str,
        root: &oci::Root,
        bundle_path: &str,
        rootfs_mounts: &[Mount],
        annotations: &HashMap<String, String>,
    ) -> Result<Arc<dyn Rootfs>> {
        self.rootfs_resource
            .handler_rootfs(
                self.device_manager.as_ref(),
                self.hypervisor.as_ref(),
                &self.sid,
                cid,
                root,
                bundle_path,
                rootfs_mounts,
                annotations,
            )
            .await
    }

    pub async fn handler_volumes(
        &self,
        cid: &str,
        spec: &oci::Spec,
    ) -> Result<Vec<Arc<dyn Volume>>> {
        let ctx = crate::volume::VolumeContext {
            d: self.device_manager.as_ref(),
            sid: &self.sid,
            agent: self.agent.clone(),
            emptydir_mode: &self.toml_config.runtime.emptydir_mode,
        };
        self.volume_resource.handler_volumes(&ctx, cid, spec).await
    }

    pub async fn handler_devices(&self, _cid: &str, linux: &Linux) -> Result<Vec<Device>> {
        // Check the entire request before attaching the first block device.
        kata_types::device::validate_linux_device_features(linux)?;
        for d in linux
            .devices()
            .iter()
            .flatten()
            .filter(|d| d.typ() == LinuxDeviceType::C)
        {
            let host_path = get_host_path(DEVICE_TYPE_CHAR, d.major(), d.minor())
                .context("resolve character device")?;
            kata_types::device::validate_device_path(std::path::Path::new(&host_path))?;
        }
        let mut devices = Vec::new();
        for d in linux
            .devices()
            .iter()
            .flatten()
            .filter(|d| d.typ() == LinuxDeviceType::B)
        {
            let blkdev_info = get_block_device_info(&self.device_manager).await;
            // Read-only intent comes from the cgroup device access rule.
            // Also honor the host device's own read-only flag (BLKROGET):
            // block-mode volumes frequently carry no read-only signal in
            // the OCI spec, so the device flag is the only reliable
            // source. Either signal being positive marks it read-only.
            let is_readonly =
                device_cgroup_access_is_readonly(linux, LinuxDeviceType::B, d.major(), d.minor())
                    || block_device_node_is_readonly(d.major(), d.minor());
            // FC's pre-created drive slots are read-write; PATCH cannot change
            // is_read_only. Do not silently expose a requested read-only raw disk
            // as writable (guest cgroup v2 does not enforce device access here).
            anyhow::ensure!(
                !is_readonly,
                "kata-fc: read-only raw block devices are unsupported by the Firecracker drive pool"
            );
            let dev_info = DeviceConfig::BlockCfgModern(BlockConfigModern {
                major: d.major(),
                minor: d.minor(),
                is_readonly,
                driver_option: blkdev_info.block_device_driver,
                ..Default::default()
            });

            let device_info = do_handle_device(&self.device_manager, &dev_info)
                .await
                .context("do handle device")?;

            // Firecracker block devices use the guest MMIO device path.
            if let DeviceType::BlockModern(device_mod) = device_info {
                let device = device_mod.lock().await.clone();
                let agent_device = Device {
                    id: device.config.virt_path.clone(),
                    container_path: d.path().display().to_string().clone(),
                    field_type: device.config.driver_option,
                    vm_path: device.config.virt_path,
                    ..Default::default()
                };
                devices.push(agent_device);
            }
        }
        Ok(devices)
    }

    pub async fn cleanup(&self) -> Result<()> {
        // Detach network endpoints.
        if let Some(network) = &self.network {
            if let Err(err) = network.remove().await {
                warn!(sl!(), "failed to remove network: {}", err);
            }
        }

        // clean up cgroup
        self.cgroups_resource
            .delete()
            .await
            .context("delete cgroup")?;

        self.volume_resource
            .cleanup_ephemeral_disks()
            .await
            .context("failed to cleanup ephemeral disks")?;

        Ok(())
    }

    pub async fn dump(&self) {
        self.rootfs_resource.dump().await;
        self.volume_resource.dump().await;
    }

    pub async fn update_linux_resource(
        &self,
        cid: &str,
        linux_resources: Option<&LinuxResources>,
        op: ResourceUpdateOp,
    ) -> Result<Option<LinuxResources>> {
        anyhow::ensure!(
            self.toml_config.runtime.static_sandbox_resource_mgmt,
            "kata-fc: dynamic VM sizing is unsupported"
        );

        // Update host cgroups while keeping the VM CPU and memory sizes fixed.
        self.cgroups_resource
            .update(cid, linux_resources, op)
            .await?;

        // update the linux resources for agent
        self.agent_linux_resources(linux_resources)
    }

    fn agent_linux_resources(
        &self,
        linux_resources: Option<&LinuxResources>,
    ) -> Result<Option<LinuxResources>> {
        let mut resources = match linux_resources {
            Some(linux_resources) => linux_resources.clone(),
            None => {
                return Ok(None);
            }
        };

        // clear the cpuset
        // for example, if there are only 5 vcpus now, and the cpuset in LinuxResources is 0-2,6, guest os will report
        // error when creating the container. so we choose to clear the cpuset here.
        if let Some(cpu) = &mut resources.cpu_mut() {
            cpu.set_cpus(None);
        }

        Ok(Some(resources))
    }
}

#[async_trait]
impl Persist for ResourceManagerInner {
    type State = ResourceState;
    type ConstructorArgs = ManagerArgs;

    /// Save a state of ResourceManagerInner
    async fn save(&self) -> Result<Self::State> {
        let cgroup_state = self.cgroups_resource.save().await?;
        Ok(ResourceState {
            cgroup_state: Some(cgroup_state),
        })
    }

    /// Restore ResourceManagerInner
    async fn restore(
        resource_args: Self::ConstructorArgs,
        resource_state: Self::State,
    ) -> Result<Self> {
        let args = CgroupArgs {
            sid: resource_args.sid.clone(),
            config: resource_args.config,
        };

        let device_manager = Arc::new(RwLock::new(
            DeviceManager::new(resource_args.hypervisor.clone()).await?,
        ));

        Ok(Self {
            sid: resource_args.sid,
            agent: resource_args.agent,
            hypervisor: resource_args.hypervisor,
            device_manager,
            network: None,
            rootfs_resource: RootFsResource::new(),
            volume_resource: VolumeResource::new(),
            cgroups_resource: CgroupsResource::restore(
                args,
                resource_state.cgroup_state.unwrap_or_default(),
            )
            .await?,
            toml_config: Arc::new(TomlConfig::default()),
        })
    }
}

/// Derive a device's read-only intent from the cgroup device access rules.
///
/// Block-mode volumes (e.g. Kubernetes volumeDevices) are passed as device
/// nodes in `spec.Linux.Devices` and carry no mount "ro" option; their
/// read-only intent is expressed solely through the cgroup device access in
/// `spec.Linux.Resources.Devices` ("rm" = read+mknod, no write, for read-only;
/// "rwm" for read-write).
///
/// The allow rule that exactly matches the device (type and exact major/minor)
/// decides: the device is read-only when that rule grants access without the
/// write ("w") bit. Wildcard rules (no major/minor) describe broad device
/// classes and are ignored so they cannot override a specific device's access.
/// If no exact rule matches, the device is left read-write.
fn device_cgroup_access_is_readonly(
    linux: &Linux,
    dev_type: LinuxDeviceType,
    major: i64,
    minor: i64,
) -> bool {
    let devices = match linux
        .resources()
        .as_ref()
        .and_then(|r| r.devices().as_ref())
    {
        Some(devices) => devices,
        None => return false,
    };

    for r in devices.iter() {
        if !r.allow() {
            continue;
        }
        let (rule_major, rule_minor) = match (r.major(), r.minor()) {
            (Some(major), Some(minor)) => (major, minor),
            _ => continue,
        };
        if rule_major != major || rule_minor != minor {
            continue;
        }
        // A specific type must match; `A` (all) and an unset type are wildcards.
        if let Some(typ) = r.typ() {
            if typ != LinuxDeviceType::A && typ != dev_type {
                continue;
            }
        }

        return !r.access().as_deref().unwrap_or("").contains('w');
    }

    false
}

/// block_device_node_is_readonly reports whether the host block device
/// identified by major:minor advertises the read-only flag (BLKROGET). This is
/// the ground truth for a device's writability: block-mode volumes frequently
/// carry no read-only signal in the OCI spec, so the device flag is the only
/// reliable source. Any failure is logged and treated as not-read-only so it
/// can never flip a positive signal back.
fn block_device_node_is_readonly(major: i64, minor: i64) -> bool {
    let host_path = match get_host_path(DEVICE_TYPE_BLOCK, major, minor) {
        Ok(path) if !path.is_empty() => path,
        Ok(_) => return false,
        Err(e) => {
            warn!(
                sl!(),
                "could not resolve host path for block device {}:{}: {:?}", major, minor, e
            );
            return false;
        }
    };

    is_block_device_readonly(&host_path).unwrap_or_else(|e| {
        warn!(
            sl!(),
            "could not query block device read-only flag for {}: {:?}", host_path, e
        );
        false
    })
}

#[cfg(test)]
mod tests {
    use super::device_cgroup_access_is_readonly;
    use oci_spec::runtime::{
        Linux, LinuxBuilder, LinuxDeviceCgroup, LinuxDeviceCgroupBuilder, LinuxDeviceType,
        LinuxResourcesBuilder,
    };
    use rstest::rstest;

    const MAJOR: i64 = 8;
    const MINOR: i64 = 0;

    fn rule(
        allow: bool,
        typ: LinuxDeviceType,
        major: Option<i64>,
        minor: Option<i64>,
        access: &str,
    ) -> LinuxDeviceCgroup {
        let mut builder = LinuxDeviceCgroupBuilder::default()
            .allow(allow)
            .typ(typ)
            .access(access);
        if let Some(major) = major {
            builder = builder.major(major);
        }
        if let Some(minor) = minor {
            builder = builder.minor(minor);
        }
        builder.build().unwrap()
    }

    fn linux_with_rules(rules: Vec<LinuxDeviceCgroup>) -> Linux {
        LinuxBuilder::default()
            .resources(
                LinuxResourcesBuilder::default()
                    .devices(rules)
                    .build()
                    .unwrap(),
            )
            .build()
            .unwrap()
    }

    #[rstest]
    #[case::no_rules(vec![], false)]
    #[case::exact_match_rm(vec![rule(true, LinuxDeviceType::B, Some(MAJOR), Some(MINOR), "rm")], true)]
    #[case::exact_match_r(vec![rule(true, LinuxDeviceType::B, Some(MAJOR), Some(MINOR), "r")], true)]
    #[case::exact_match_rwm(vec![rule(true, LinuxDeviceType::B, Some(MAJOR), Some(MINOR), "rwm")], false)]
    #[case::type_all_is_wildcard(vec![rule(true, LinuxDeviceType::A, Some(MAJOR), Some(MINOR), "rm")], true)]
    #[case::deny_rule_ignored(vec![rule(false, LinuxDeviceType::B, Some(MAJOR), Some(MINOR), "rm")], false)]
    #[case::wildcard_major_ignored(vec![rule(true, LinuxDeviceType::B, None, Some(MINOR), "rm")], false)]
    #[case::wildcard_minor_ignored(vec![rule(true, LinuxDeviceType::B, Some(MAJOR), None, "rm")], false)]
    #[case::type_mismatch_ignored(vec![rule(true, LinuxDeviceType::C, Some(MAJOR), Some(MINOR), "rm")], false)]
    #[case::different_device_ignored(vec![rule(true, LinuxDeviceType::B, Some(9), Some(1), "rm")], false)]
    #[case::first_exact_match_wins(
        vec![
            rule(true, LinuxDeviceType::B, Some(MAJOR), Some(MINOR), "rm"),
            rule(true, LinuxDeviceType::B, Some(MAJOR), Some(MINOR), "rwm"),
        ],
        true
    )]
    fn test_device_cgroup_access_is_readonly(
        #[case] rules: Vec<LinuxDeviceCgroup>,
        #[case] expected: bool,
    ) {
        let linux = linux_with_rules(rules);
        assert_eq!(
            device_cgroup_access_is_readonly(&linux, LinuxDeviceType::B, MAJOR, MINOR),
            expected
        );
    }

    #[test]
    fn test_no_resources() {
        let linux = LinuxBuilder::default().build().unwrap();
        assert!(!device_cgroup_access_is_readonly(
            &linux,
            LinuxDeviceType::B,
            MAJOR,
            MINOR
        ));
    }
}

#[cfg(test)]
mod network_thread_tests {
    use super::run_network_thread;
    use anyhow::{anyhow, Context, Result};
    use std::{sync::mpsc, thread, time::Duration};

    // One scheduler thread makes an inline blocking join fail deterministically:
    // the OS thread cannot receive until that scheduler runs the pending task.
    #[tokio::test(flavor = "current_thread")]
    async fn network_thread_wait_allows_runtime_progress() {
        let runtime = tokio::runtime::Handle::current();
        let value = tokio::spawn(async move {
            let runtime_thread = thread::current().id();
            run_network_thread(move || {
                assert_ne!(thread::current().id(), runtime_thread);
                let (sender, receiver) = mpsc::channel();
                runtime.spawn(async move {
                    let _ = sender.send(42);
                });
                // Bound the failure case independently of Tokio's blocked timer.
                receiver
                    .recv_timeout(Duration::from_secs(5))
                    .context("outer runtime did not make progress")
            })
            .await
        })
        .await
        .unwrap()
        .unwrap();
        assert_eq!(value, 42);
    }

    #[tokio::test]
    async fn network_thread_preserves_errors_and_panics() {
        let error = run_network_thread(|| -> Result<()> { Err(anyhow!("setup failed")) })
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "setup failed");

        let error = run_network_thread(|| -> Result<()> { panic!("network thread panic") })
            .await
            .unwrap_err();
        assert!(error
            .to_string()
            .contains("Couldn't join on the associated thread"));
    }
}

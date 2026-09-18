// Copyright (c) 2019-2022 Alibaba Cloud
// Copyright (c) 2019-2022 Ant Group
//
// SPDX-License-Identifier: Apache-2.0
//

use std::{collections::HashMap, sync::Arc, thread};

use agent::{ARPNeighbor, Agent, Storage};
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use hypervisor::{
    device::{
        device_manager::{do_handle_device, DeviceManager},
        util::{get_host_path, DEVICE_TYPE_CHAR},
        DeviceConfig,
    },
    Hypervisor,
};
use kata_types::mount::{kata_guest_sandbox_dir, Mount, KATA_EPHEMERAL_VOLUME_TYPE, SHM_DIR};
use kata_types::{
    config::TomlConfig,
    mount::{adjust_rootfs_mounts, KATA_IMAGE_FORCE_GUEST_PULL},
};
use libc::NUD_PERMANENT;
use oci::{Linux, LinuxResources};
use oci_spec::runtime::{self as oci, LinuxDeviceType};
use persist::sandbox_persist::Persist;
use tokio::{runtime, sync::RwLock};

use crate::{
    cgroups::{CgroupArgs, CgroupsResource},
    cpu_mem::{cpu::CpuResource, initial_size::InitialSizeManager, mem::MemResource},
    manager::ManagerArgs,
    network::{self, Network, NetworkConfig, NetworkWithNetNsConfig},
    resource_persist::ResourceState,
    rootfs::{RootFsResource, Rootfs},
    volume::{Volume, VolumeResource},
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
    pub cpu_resource: CpuResource,
    pub mem_resource: MemResource,
}

impl ResourceManagerInner {
    pub(crate) async fn new(
        sid: &str,
        agent: Arc<dyn Agent>,
        hypervisor: Arc<dyn Hypervisor>,
        toml_config: Arc<TomlConfig>,
        init_size_manager: InitialSizeManager,
    ) -> Result<Self> {
        // create device manager
        let dev_manager = DeviceManager::new(hypervisor.clone())
            .await
            .context("failed to create device manager")?;
        let device_manager = Arc::new(RwLock::new(dev_manager));

        let cgroups_resource = CgroupsResource::new(sid, &toml_config)?;
        let cpu_resource = CpuResource::new(toml_config.clone())?;
        let mem_resource = MemResource::new(init_size_manager)?;
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
            cpu_resource,
            mem_resource,
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
                ResourceConfig::GuestExtensionImage(r) => {
                    do_handle_device(&self.device_manager, &DeviceConfig::BlockCfgModern(r))
                        .await
                        .context("do handle extra image device failed.")?;
                }
                ResourceConfig::HybridVsock(hv) => {
                    do_handle_device(&self.device_manager, &DeviceConfig::HybridVsockCfg(hv))
                        .await
                        .context("do handle hybrid-vsock device failed.")?;
                }
                ResourceConfig::InitData(id) => {
                    do_handle_device(&self.device_manager, &DeviceConfig::BlockCfgModern(id))
                        .await
                        .context("do handle initdata block device failed.")?;
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
        let network = thread::spawn(move || -> Result<Arc<dyn Network>> {
            let rt = runtime::Builder::new_current_thread().enable_io().build()?;
            let d = rt
                .block_on(network::new(&network_config, device_manager))
                .context("new network")?;
            rt.block_on(d.setup()).context("setup network")?;
            Ok(d)
        })
        .join()
        .map_err(|e| anyhow!("{:?}", e))
        .context("Couldn't join on the associated thread")?
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
            .setup_after_start_vm(self.hypervisor.as_ref())
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

    /// Check whether a rescan is needed at all (early-out conditions).
    pub fn rescan_should_skip(&self, net_cfg: &NetworkWithNetNsConfig) -> bool {
        self.toml_config.runtime.disable_new_netns
            || net_cfg.network_model == "none"
            || net_cfg.netns_path.is_empty()
    }

    /// Check whether the network already has interfaces configured.
    pub async fn network_has_interfaces(&self) -> Result<bool> {
        match self.network.as_ref() {
            Some(n) => Ok(!n
                .interfaces()
                .await
                .context("check existing interfaces")?
                .is_empty()),
            None => Ok(false),
        }
    }

    /// Perform a single network scan attempt.  Returns `Some(network)` when
    /// new interfaces were found and need to be applied to the guest agent,
    /// `None` when no interfaces were found yet (caller should retry).
    /// The caller is responsible for calling `apply_network_to_agent` on
    /// the returned network **after** releasing the write lock.
    pub async fn rescan_network_once(
        &mut self,
        net_cfg: NetworkWithNetNsConfig,
    ) -> Result<Option<Arc<dyn Network>>> {
        self.handle_network(NetworkConfig::NetNs(net_cfg))
            .await
            .context("rescan handle network")?;

        let n = self
            .network
            .as_ref()
            .ok_or_else(|| anyhow!("network missing after rescan setup"))?;
        let ifs = n.interfaces().await.context("rescan get interfaces")?;
        if !ifs.is_empty() {
            return Ok(Some(Arc::clone(n)));
        }
        Ok(None)
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
        let adjust_rootfs_mounts = if !self
            .config()
            .runtime
            .is_experiment_enabled(KATA_IMAGE_FORCE_GUEST_PULL)
        {
            rootfs_mounts.to_vec()
        } else {
            adjust_rootfs_mounts()?
        };

        self.rootfs_resource
            .handler_rootfs(
                self.device_manager.as_ref(),
                self.hypervisor.as_ref(),
                &self.sid,
                cid,
                root,
                bundle_path,
                &adjust_rootfs_mounts,
                annotations,
            )
            .await
    }

    pub async fn handler_volumes(
        &self,
        cid: &str,
        spec: &oci::Spec,
    ) -> Result<Vec<Arc<dyn Volume>>> {
        let capabilities = self.hypervisor.capabilities().await?;
        let ctx = crate::volume::VolumeContext {
            d: self.device_manager.as_ref(),
            sid: &self.sid,
            agent: self.agent.clone(),
            emptydir_mode: &self.toml_config.runtime.emptydir_mode,
            fs_sharing_supported: capabilities.is_fs_sharing_supported(),
            block_device_discard_supported: capabilities.is_block_device_discard_supported(),
        };
        self.volume_resource.handler_volumes(&ctx, cid, spec).await
    }

    pub fn validate_devices(&self, linux: &Linux) -> Result<()> {
        // Retain host character-device validation; raw block nodes are rejected.
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
        Ok(())
    }

    pub async fn cleanup(&self) -> Result<()> {
        // Detach network endpoints.
        if let Some(network) = &self.network {
            if let Err(err) = network.remove(self.hypervisor.as_ref()).await {
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

        // we should firstly update the vcpus and mems, and then update the host cgroups
        self.cgroups_resource
            .update(cid, linux_resources, op, self.hypervisor.as_ref())
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
        let mut endpoint_state = vec![];
        if let Some(network) = &self.network {
            if let Some(ens) = network.save().await {
                endpoint_state = ens;
            }
        }
        let cgroup_state = self.cgroups_resource.save().await?;
        Ok(ResourceState {
            endpoint: endpoint_state,
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

        let mem_resource = MemResource::default();
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
            cpu_resource: CpuResource::default(),
            mem_resource,
        })
    }
}

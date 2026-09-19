// Copyright (c) 2019-2022 Alibaba Cloud
// Copyright (c) 2019-2022 Ant Group
//
// SPDX-License-Identifier: Apache-2.0
//

use crate::health_check::HealthCheck;
use agent::kata::KataAgent;
use agent::{self, Agent, VolumeStatsRequest};
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use common::error::is_normal_oom_shutdown_error;
use common::types::utils::option_system_time_into;
use common::types::ContainerProcess;
use common::{
    message::{Action, Message},
    types::DEFAULT_SHM_SIZE,
};
use common::{
    types::{SandboxConfig, SandboxExitInfo, SandboxStatus},
    ContainerManager, Sandbox, SandboxNetworkEnv,
};

use containerd_shim_protos::events::task::{TaskExit, TaskOOM};

#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
use hypervisor::{firecracker::Firecracker, HYPERVISOR_FIRECRACKER};
use hypervisor::{BlockConfigModern, Hypervisor};

use hypervisor::{
    utils::{get_hvsock_path, remove_vmm_user_runtime_dir, vmm_user_runtime_dir},
    HybridVsockConfig, DEFAULT_GUEST_VSOCK_CID,
};
use kata_sys_util::spec::load_oci_spec;

use kata_types::config::TomlConfig;
use persist::{self, sandbox_persist::Persist};
use protobuf::SpecialFields;
use resource::manager::ManagerArgs;
use resource::network::{NetworkConfig, NetworkWithNetNsConfig};
use resource::{ResourceConfig, ResourceManager};
use std::sync::Arc;
use std::time::SystemTime;
use strum::Display;
use tokio::sync::{mpsc::Sender, watch, Mutex, RwLock};
use tokio_util::sync::CancellationToken;
use tracing::instrument;

pub(crate) const VIRTCONTAINER: &str = "virt_container";

pub struct SandboxRestoreArgs {
    pub sid: String,
    pub toml_config: TomlConfig,
    pub sender: Sender<Message>,
}

#[derive(Clone, Copy, PartialEq, Debug, Display)]
pub enum SandboxState {
    Init,
    Running,
    Stopped,
}

impl SandboxState {
    fn to_cri_state(self) -> &'static str {
        match self {
            SandboxState::Running => "SANDBOX_READY",
            SandboxState::Init | SandboxState::Stopped => "SANDBOX_NOTREADY",
        }
    }
}

struct SandboxInner {
    state: SandboxState,
    exit_info: Option<SandboxExitInfo>,
    created_at: Option<SystemTime>,
    // Whether sandbox resources (cgroup, network, mounts, ...) have already
    // been released.  Teardown can be driven both by the sandbox container
    // exiting and by an explicit shutdown RPC, so guard against running the
    // cleanup twice.
    cleaned: bool,
}

impl SandboxInner {
    pub fn new() -> Self {
        Self {
            state: SandboxState::Init,
            exit_info: None,
            created_at: None,
            cleaned: false,
        }
    }
}

#[derive(Clone)]
pub struct VirtSandbox {
    sid: String,
    msg_sender: Arc<Mutex<Sender<Message>>>,
    inner: Arc<RwLock<SandboxInner>>,
    resource_manager: Arc<ResourceManager>,
    agent: Arc<dyn Agent>,
    hypervisor: Arc<dyn Hypervisor>,
    monitor: Arc<HealthCheck>,
    exit_notify_tx: watch::Sender<bool>,
    sandbox_config: Option<SandboxConfig>,
    shm_size: u64,
    cancel_token: CancellationToken,
    pub(crate) oom_registry: Arc<crate::oom::OomRegistry>,
}

impl std::fmt::Debug for VirtSandbox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VirtSandbox")
            .field("sid", &self.sid)
            .field("msg_sender", &self.msg_sender)
            .field("inner", &"<SandboxInner>")
            .field("resource_manager", &self.resource_manager)
            .field("agent", &"<Agent>")
            .field("hypervisor", &self.hypervisor)
            .field("monitor", &"<HealthCheck>")
            .field("exit_notify_tx", &"<watch::Sender<bool>>")
            .field("sandbox_config", &self.sandbox_config)
            .finish()
    }
}

impl VirtSandbox {
    pub async fn new(
        sid: &str,
        msg_sender: Sender<Message>,
        agent: Arc<dyn Agent>,
        hypervisor: Arc<dyn Hypervisor>,
        resource_manager: Arc<ResourceManager>,
        sandbox_config: SandboxConfig,
    ) -> Result<Self> {
        let config = resource_manager.config().await;
        let keep_abnormal = config.runtime.keep_abnormal;
        let (exit_notify_tx, _) = watch::channel(false);
        let cancel_token = CancellationToken::new();
        Ok(Self {
            sid: sid.to_string(),
            msg_sender: Arc::new(Mutex::new(msg_sender)),
            inner: Arc::new(RwLock::new(SandboxInner::new())),
            agent,
            hypervisor,
            resource_manager,
            monitor: Arc::new(HealthCheck::new(true, keep_abnormal)),
            exit_notify_tx,
            shm_size: sandbox_config.shm_size,
            sandbox_config: Some(sandbox_config),
            cancel_token,
            oom_registry: Default::default(),
        })
    }

    pub fn get_agent(&self) -> Arc<dyn Agent> {
        self.agent.clone()
    }

    pub fn get_sid(&self) -> String {
        self.sid.clone()
    }

    pub fn get_hypervisor(&self) -> Arc<dyn Hypervisor> {
        self.hypervisor.clone()
    }

    async fn record_stop(&self, exit_status: u32, exited_at: std::time::SystemTime) {
        let mut inner = self.inner.write().await;
        if inner.state == SandboxState::Stopped {
            return;
        }

        inner.state = SandboxState::Stopped;
        inner.exit_info = Some(SandboxExitInfo {
            exit_status,
            exited_at: Some(exited_at),
        });
        let _ = self.exit_notify_tx.send(true);
    }

    #[instrument]
    async fn prepare_for_start_sandbox(
        &self,
        id: &str,
        sandbox_config: &SandboxConfig,
    ) -> Result<Vec<ResourceConfig>> {
        let config = self.resource_manager.config().await;
        resource::network::reject_dan(&config, &self.sid)?;
        let mut resource_configs = vec![];

        info!(sl!(), "prepare vm socket config for sandbox.");
        let vm_socket_config = self
            .prepare_vm_socket_config()
            .await
            .context("failed to prepare vm socket config")?;
        resource_configs.push(vm_socket_config);

        let network_env: SandboxNetworkEnv = sandbox_config.network_env.clone();
        // prepare network config
        if !network_env.network_created {
            if let Some(network_resource) = self.prepare_network_resource(&network_env).await {
                resource_configs.push(network_resource);
            }
        }

        // prepare VM rootfs device config
        if let Some(block_config) = self
            .prepare_rootfs_config()
            .await
            .context("failed to prepare rootfs device config")?
        {
            let vm_rootfs = ResourceConfig::VmRootfs(block_config);
            resource_configs.push(vm_rootfs);
        }

        Ok(resource_configs)
    }

    async fn prepare_network_resource(
        &self,
        network_env: &SandboxNetworkEnv,
    ) -> Option<ResourceConfig> {
        let config = self.resource_manager.config().await;
        if let Some(netns_path) = network_env.netns.as_ref() {
            Some(ResourceConfig::Network(NetworkConfig::NetNs(
                NetworkWithNetNsConfig {
                    network_model: config.runtime.internetworking_model.clone(),
                    netns_path: netns_path.to_owned(),
                    queues: self
                        .hypervisor
                        .hypervisor_config()
                        .await
                        .network_info
                        .network_queues as usize,
                    network_created: network_env.network_created,
                },
            )))
        } else {
            None
        }
    }

    async fn prepare_rootfs_config(&self) -> Result<Option<BlockConfigModern>> {
        let boot_info = self.hypervisor.hypervisor_config().await.boot_info;

        if !boot_info.initrd.is_empty() {
            return Ok(None);
        }

        if boot_info.image.is_empty() {
            return Err(anyhow!("both image and initrd are unset"));
        }

        Ok(Some(BlockConfigModern {
            path_on_host: boot_info.image.clone(),
            is_readonly: true,
            driver_option: boot_info.vm_rootfs_driver,
            ..Default::default()
        }))
    }

    async fn prepare_vm_socket_config(&self) -> Result<ResourceConfig> {
        // This fork has exactly one VMM and one agent transport.
        Ok(ResourceConfig::HybridVsock(HybridVsockConfig {
            guest_cid: DEFAULT_GUEST_VSOCK_CID,
            uds_path: get_hvsock_path(&self.sid),
        }))
    }

    /// Build a network rescan config targeting the hypervisor's network
    /// namespace.  Docker 26+ bind-mounts `/proc/<vmm_pid>/ns/net` and
    /// configures veth pairs there between Create and Start, so the
    /// hypervisor netns is where the interfaces will appear — regardless
    /// of whether we earlier created a placeholder netns (network_created)
    /// or not.  This mirrors the Go shim's `detectHypervisorNetns` logic
    /// inside `addAllEndpoints` (commit f7878cc).
    async fn netns_rescan_config(&self) -> Option<NetworkWithNetNsConfig> {
        let toml = self.resource_manager.config().await;
        if toml.runtime.disable_new_netns {
            return None;
        }

        self.sandbox_config.as_ref()?;

        let vmm_pid = match self.hypervisor.get_vmm_master_tid().await {
            Ok(pid) => pid,
            Err(e) => {
                warn!(sl!(), "netns_rescan_config: cannot get VMM PID: {:?}", e);
                return None;
            }
        };
        let netns_path = format!("/proc/{}/ns/net", vmm_pid);

        let queues = self
            .hypervisor
            .hypervisor_config()
            .await
            .network_info
            .network_queues as usize;
        Some(NetworkWithNetNsConfig {
            network_model: toml.runtime.internetworking_model.clone(),
            netns_path,
            queues,
            network_created: false,
        })
    }
}

#[async_trait]
impl Sandbox for VirtSandbox {
    #[instrument(name = "sb: start")]
    async fn start(&self) -> Result<()> {
        let id = &self.sid;

        if self.sandbox_config.is_none() {
            return Err(anyhow!("sandbox config is missing"));
        }
        let sandbox_config = self.sandbox_config.as_ref().unwrap();

        // if sandbox is not in SandboxState::Init then return,
        // otherwise try to create sandbox

        let mut inner = self.inner.write().await;
        if inner.state != SandboxState::Init {
            warn!(sl!(), "sandbox is started");
            return Ok(());
        }
        let selinux_label = load_oci_spec().ok().and_then(|spec| {
            spec.process()
                .as_ref()
                .and_then(|process| process.selinux_label().clone())
        });

        self.hypervisor
            .prepare_vm(
                id,
                sandbox_config.network_env.netns.clone(),
                &sandbox_config.annotations,
                selinux_label,
            )
            .await
            .context("prepare vm")?;

        // generate device and setup before start vm
        // should after hypervisor.prepare_vm
        let resources = self.prepare_for_start_sandbox(id, sandbox_config).await?;

        self.resource_manager
            .prepare_before_start_vm(resources)
            .await
            .context("set up device before start vm")?;

        // start vm
        self.hypervisor.start_vm(10_000).await.context("start vm")?;
        info!(sl!(), "start vm");

        let sandbox = self.clone();
        // wait for vm exit in background, and record the exit status and time when vm exited.
        tokio::spawn(async move {
            match sandbox.hypervisor.wait_vm().await {
                Ok(exit_code) => {
                    sandbox
                        .record_stop(exit_code as u32, SystemTime::now())
                        .await;
                }
                Err(err) => {
                    warn!(sl!(), "failed waiting for sandbox VM exit: {:?}", err);
                    sandbox.record_stop(255, SystemTime::now()).await;
                }
            }
        });

        // connect agent
        // set agent socket
        let address = self
            .hypervisor
            .get_agent_socket()
            .await
            .context("get agent socket")?;
        self.agent
            .start(&address)
            .await
            .context(format!("connect to address {:?}", &address))?;

        self.resource_manager
            .setup_after_start_vm()
            .await
            .context("setup device after start vm")?;

        // create sandbox in vm
        let req = agent::CreateSandboxRequest {
            hostname: sandbox_config.hostname.clone(),
            dns: sandbox_config.dns.clone(),
            storages: self
                .resource_manager
                .get_storage_for_sandbox(self.shm_size)
                .await
                .context("get storages for sandbox")?,
            sandbox_pidns: false,
            sandbox_id: id.to_string(),
        };

        self.agent
            .create_sandbox(req)
            .await
            .context("create sandbox")?;

        inner.state = SandboxState::Running;
        inner.created_at = Some(std::time::SystemTime::now());

        let agent = self.agent.clone();
        let sender = self.msg_sender.clone();
        let cancel_token = self.cancel_token.clone();

        let oom_registry = self.oom_registry.clone();
        info!(sl!(), "oom watcher start");
        tokio::spawn(async move {
            let mut requests = tokio::time::interval(std::time::Duration::from_millis(100));
            requests.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! { _ = cancel_token.cancelled() => break, _ = requests.tick() => {} }
                tokio::select! {
                    _ = cancel_token.cancelled() => {
                        // Sandbox or VM is shutting down, gracefully exit watcher
                        info!(sl!(), "oom watcher cancelled, sandbox is stopping");
                        break;
                    }
                    res = agent.get_oom_event(agent::Empty::new()) => {
                        match res.context("get oom event") {
                            Ok(resp) => {
                                let cid = &resp.container_id;
                                if !oom_registry.accept(cid).await { continue; }
                                warn!(sl!(), "send oom event for container {}", &cid);
                                let event = TaskOOM {
                                    container_id: cid.to_string(),
                                    ..Default::default()
                                };
                                let msg = Message::new(Action::Event(Arc::new(event)));
                                let lock_sender = sender.lock().await;
                                let sent = tokio::select! {
                                    _ = cancel_token.cancelled() => break,
                                    result = tokio::time::timeout(std::time::Duration::from_secs(1), lock_sender.send(msg)) => result,
                                };
                                if !matches!(sent, Ok(Ok(()))) {
                                    error!(
                                        sl!(),
                                        "failed to send bounded oom event for {}", cid
                                    );
                                }
                            }
                            Err(err) => {
                                // Handle errors by type
                                if is_normal_oom_shutdown_error(&err) {
                                    info!(sl!(), "oom watcher exit on sandbox shutdown: {:?}", err);
                                    break;
                                } else {
                                    warn!(sl!(), "failed to get oom event error {:?}", err);
                                    tokio::select! { _ = cancel_token.cancelled() => break, _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => {} }
                                    continue;
                                }
                            }
                        }
                    }
                }
            }
        });

        self.monitor.start(id, self.agent.clone());
        self.save().await.context("save state")?;

        Ok(())
    }

    /// Core function for starting a VM from a template
    ///
    /// This function is responsible for creating and starting a VM sandbox from a predefined template,
    /// serving as the core implementation of the template mechanism.
    async fn start_template(&self) -> Result<()> {
        info!(sl!(), "sandbox::start_template()"; "sandbox:" => format!("{:?}", self));
        let id = &self.sid;

        let sandbox_config = self.sandbox_config.as_ref().unwrap();

        // if sandbox is not in SandboxState::Init then return,
        // otherwise try to create sandbox
        let inner = self.inner.write().await;
        if inner.state != SandboxState::Init {
            return Ok(());
        }
        let selinux_label = load_oci_spec().ok().and_then(|spec| {
            spec.process()
                .as_ref()
                .and_then(|process| process.selinux_label().clone())
        });

        self.hypervisor
            .prepare_vm(
                id,
                sandbox_config.network_env.netns.clone(),
                &sandbox_config.annotations,
                selinux_label,
            )
            .await
            .context("prepare vm")?;

        // generate device and setup before start vm
        // should after hypervisor.prepare_vm
        let resources = self
            .prepare_for_start_sandbox(id, sandbox_config)
            .await
            .context("prepare resources before start vm")?;

        self.resource_manager
            .prepare_before_start_vm(resources)
            .await
            .context("set up device before start vm")?;

        self.hypervisor
            .start_vm(10_000)
            .await
            .context("start template vm")?;
        info!(sl!(), "vm started from template");

        let sandbox = self.clone();
        tokio::spawn(async move {
            match sandbox.hypervisor.wait_vm().await {
                Ok(exit_code) => {
                    sandbox
                        .record_stop(exit_code as u32, SystemTime::now())
                        .await;
                }
                Err(err) => {
                    warn!(sl!(), "failed waiting for sandbox VM exit: {:?}", err);
                    sandbox.record_stop(255, SystemTime::now()).await;
                }
            }
        });

        Ok(())
    }

    async fn status(&self) -> Result<SandboxStatus> {
        let inner = self.inner.read().await;
        let state = inner.state.to_cri_state().to_string();

        Ok(SandboxStatus {
            sandbox_id: self.sid.clone(),
            pid: std::process::id(),
            state,
            info: std::collections::HashMap::new(),
            created_at: inner.created_at,
        })
    }

    async fn wait(&self) -> Result<SandboxExitInfo> {
        info!(sl!(), "wait sandbox");
        {
            let inner = self.inner.read().await;
            if inner.state == SandboxState::Stopped {
                return Ok(inner.exit_info.clone().unwrap_or_default());
            }
        }

        let mut exit_notify_rx = self.exit_notify_tx.subscribe();
        while !*exit_notify_rx.borrow() {
            exit_notify_rx
                .changed()
                .await
                .context("wait for sandbox stop notification")?;
        }

        let inner = self.inner.read().await;
        Ok(inner.exit_info.clone().unwrap_or_default())
    }

    async fn stop(&self) -> Result<()> {
        let state = {
            let sandbox_inner = self.inner.read().await;
            sandbox_inner.state
        };

        if state == SandboxState::Stopped {
            return Ok(());
        }

        // Cancel the OOM watcher before tearing down the VM so it exits
        // cleanly instead of hitting ECONNRESET/EOF on a closed channel.
        self.cancel_token.cancel();

        info!(sl!(), "begin stop sandbox");
        if state == SandboxState::Init {
            let _ = self.hypervisor.stop_vm().await;
            self.record_stop(0, SystemTime::now()).await;
            info!(sl!(), "sandbox stopped during Init");
            return Ok(());
        }

        self.hypervisor.stop_vm().await.context("stop vm")?;
        self.wait().await.context("wait for vm exit after stop")?;
        info!(sl!(), "sandbox stopped");

        Ok(())
    }

    async fn shutdown(&self) -> Result<()> {
        info!(sl!(), "shutdown");

        self.stop().await.context("stop")?;

        self.cleanup().await.context("do the clean up")?;

        info!(sl!(), "stop monitor");
        self.monitor.stop().await;

        info!(sl!(), "stop agent");
        self.agent.stop().await;

        // stop server
        info!(sl!(), "send shutdown message");
        let msg = Message::new(Action::Shutdown);
        let sender = self.msg_sender.clone();
        let sender = sender.lock().await;
        sender.send(msg).await.context("send shutdown msg")?;
        Ok(())
    }

    async fn cleanup(&self) -> Result<()> {
        // Teardown may be triggered both when the sandbox container exits and
        // by a later shutdown RPC; only release the resources once.
        {
            let mut inner = self.inner.write().await;
            if inner.cleaned {
                return Ok(());
            }
            inner.cleaned = true;
        }

        let rootless_uid = self
            .hypervisor
            .hypervisor_config()
            .await
            .security_info
            .rootless_user
            .map(|user| user.uid);

        info!(sl!(), "delete hypervisor");
        self.hypervisor
            .cleanup()
            .await
            .context("delete hypervisor")?;

        info!(sl!(), "resource clean up");
        self.resource_manager
            .cleanup()
            .await
            .context("resource clean up")?;

        if let Some(uid) = rootless_uid {
            let path = vmm_user_runtime_dir(uid);
            if let Err(err) = remove_vmm_user_runtime_dir(uid) {
                warn!(
                    sl!(),
                    "failed to remove rootless runtime directory {}: {}",
                    path.display(),
                    err
                );
            }
        }

        // TODO: cleanup other sandbox resource
        Ok(())
    }

    async fn rescan_network(&self) -> Result<()> {
        if let Some(net_cfg) = self.netns_rescan_config().await {
            info!(
                sl!(),
                "rescan_network: scanning netns={}", net_cfg.netns_path
            );
            self.resource_manager
                .rescan_network_if_unconfigured(net_cfg)
                .await
                .context("network rescan during start")?;
        }
        Ok(())
    }

    async fn wait_process(
        &self,
        cm: Arc<dyn ContainerManager>,
        process_id: ContainerProcess,
        shim_pid: u32,
    ) -> Result<()> {
        let exit_status = cm.wait_process(&process_id).await?;
        info!(sl!(), "container process exited with {:?}", exit_status);

        if cm.is_sandbox_container(&process_id).await {
            self.stop().await.context("stop sandbox")?;
        }

        let cid = process_id.container_id();
        if cid.is_empty() {
            return Err(anyhow!("container id is empty"));
        }
        let eid = process_id.exec_id();
        let id = if eid.is_empty() {
            cid.to_string()
        } else {
            eid.to_string()
        };

        let event = TaskExit {
            container_id: cid.to_string(),
            id,
            pid: shim_pid,
            exit_status: exit_status.exit_code as u32,
            exited_at: option_system_time_into(exit_status.exit_time),
            special_fields: SpecialFields::new(),
        };
        let msg = Message::new(Action::Event(Arc::new(event)));
        let lock_sender = self.msg_sender.lock().await;
        lock_sender.send(msg).await.context("send exit event")?;
        Ok(())
    }

    async fn agent_sock(&self) -> Result<String> {
        self.agent.agent_sock().await
    }

    async fn direct_volume_stats(&self, volume_guest_path: &str) -> Result<String> {
        let req: agent::VolumeStatsRequest = VolumeStatsRequest {
            volume_guest_path: volume_guest_path.to_string(),
        };
        let result = self
            .agent
            .get_volume_stats(req)
            .await
            .context("sandbox: failed to process direct volume stats query")?;
        Ok(format!(
            "Usage: {:?} Volume Condition: {:?}",
            result.usage(),
            result.volume_condition()
        ))
    }

    async fn agent_metrics(&self) -> Result<String> {
        self.agent
            .get_metrics(agent::Empty::new())
            .await
            .map_err(|err| anyhow!("failed to get agent metrics {:?}", err))
            .map(|resp| resp.metrics)
    }
}

#[async_trait]
impl Persist for VirtSandbox {
    type State = crate::sandbox_persist::SandboxState;
    type ConstructorArgs = SandboxRestoreArgs;

    /// Save a state of Sandbox
    async fn save(&self) -> Result<Self::State> {
        let hypervisor_state = self.hypervisor.save_state().await?;
        let sandbox_state = crate::sandbox_persist::SandboxState {
            sandbox_type: VIRTCONTAINER.to_string(),
            resource: Some(self.resource_manager.save().await?),
            hypervisor: match hypervisor_state.hypervisor_type.as_str() {
                #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
                HYPERVISOR_FIRECRACKER => Ok(Some(hypervisor_state)),

                _ => Err(anyhow!(
                    "Unsupported hypervisor {}",
                    hypervisor_state.hypervisor_type
                )),
            }?,
        };
        // FIXME: properly handle jailed case
        // eg: Determine if we are running jailed:
        // let h = sandbox_state.hypervisor.clone().unwrap_or_default();
        // Figure out the jailed path:
        // jailed_path = h.<>
        // and somehow store the sandbox state into the jail:
        // persist::to_disk(&sandbox_state, &self.sid, jailed_path)?;
        // Issue is, how to handle restore.
        let h = sandbox_state.hypervisor.as_ref().unwrap();
        let vmpath = match h.jailed {
            true => h.vm_path.clone(),
            false => "".to_string(),
        };
        persist::to_disk(&sandbox_state, &self.sid, vmpath.as_str())?;
        Ok(sandbox_state)
    }
    /// Restore Sandbox
    async fn restore(
        sandbox_args: Self::ConstructorArgs,
        sandbox_state: Self::State,
    ) -> Result<Self> {
        let config = sandbox_args.toml_config;
        let r = sandbox_state.resource.unwrap_or_default();
        let h = sandbox_state.hypervisor.unwrap_or_default();
        let hypervisor = match h.hypervisor_type.as_str() {
            #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
            HYPERVISOR_FIRECRACKER => {
                let hypervisor =
                    Arc::new(Firecracker::restore((), h).await?) as Arc<dyn Hypervisor>;
                Ok(hypervisor)
            }

            _ => Err(anyhow!("Unsupported hypervisor {}", &h.hypervisor_type)),
        }?;
        let agent = Arc::new(KataAgent::new(kata_types::config::Agent::default()));
        let sid = sandbox_args.sid;
        let keep_abnormal = config.runtime.keep_abnormal;
        let args = ManagerArgs {
            sid: sid.clone(),
            agent: agent.clone(),
            hypervisor: hypervisor.clone(),
            config,
        };
        let resource_manager = Arc::new(ResourceManager::restore(args, r).await?);
        Ok(Self {
            sid: sid.to_string(),
            msg_sender: Arc::new(Mutex::new(sandbox_args.sender)),
            inner: Arc::new(RwLock::new(SandboxInner::new())),
            agent,
            hypervisor,
            resource_manager,
            monitor: Arc::new(HealthCheck::new(true, keep_abnormal)),
            exit_notify_tx: watch::channel(false).0,
            sandbox_config: None,
            shm_size: DEFAULT_SHM_SIZE,
            cancel_token: CancellationToken::default(),
            oom_registry: Default::default(),
        })
    }
}

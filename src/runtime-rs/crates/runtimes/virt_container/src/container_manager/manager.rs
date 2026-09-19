// Copyright (c) 2019-2022 Alibaba Cloud
// Copyright (c) 2019-2022 Ant Group
//
// SPDX-License-Identifier: Apache-2.0
//

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;

use std::{collections::HashMap, sync::Arc};

use agent::Agent;
use common::{
    error::Error,
    types::{
        ContainerConfig, ContainerID, ContainerProcess, ExecProcessRequest, KillRequest,
        ProcessExitStatus, ProcessStateInfo, ProcessStatus, ProcessType, ResizePTYRequest,
        ShutdownRequest, StatsInfo, UpdateRequest, PID,
    },
    ContainerManager,
};
use hypervisor::Hypervisor;
use oci::Process as OCIProcess;
use oci_spec::runtime as oci;
use resource::ResourceManager;
use tokio::sync::{OnceCell, RwLock};
use tracing::instrument;

use crate::container_manager::is_termination_signal;

use super::{logger_with_process, Container};

pub struct VirtContainerManager {
    sid: String,
    pid: u32,
    containers: Arc<RwLock<HashMap<String, Container>>>,
    resource_manager: Arc<ResourceManager>,
    agent: Arc<dyn Agent>,
    hypervisor: Arc<dyn Hypervisor>,
    vmm_master_tid: OnceCell<u32>,
    oom_registry: Arc<crate::oom::OomRegistry>,
}

impl std::fmt::Debug for VirtContainerManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VirtContainerManager")
            .field("sid", &self.sid)
            .field("pid", &self.pid)
            .finish()
    }
}

impl VirtContainerManager {
    pub fn new(
        sid: &str,
        pid: u32,
        agent: Arc<dyn Agent>,
        hypervisor: Arc<dyn Hypervisor>,
        resource_manager: Arc<ResourceManager>,
        oom_registry: Arc<crate::oom::OomRegistry>,
    ) -> Self {
        Self {
            sid: sid.to_string(),
            pid,
            containers: Default::default(),
            resource_manager,
            agent,
            hypervisor,
            vmm_master_tid: OnceCell::new(),
            oom_registry,
        }
    }

    async fn get_vmm_master_tid(&self) -> Result<u32> {
        self.vmm_master_tid
            .get_or_try_init(|| self.hypervisor.get_vmm_master_tid())
            .await
            .copied()
    }
}

#[async_trait]
impl ContainerManager for VirtContainerManager {
    #[instrument]
    async fn create_container(&self, config: ContainerConfig, spec: oci::Spec) -> Result<PID> {
        let vmm_master_tid = self.get_vmm_master_tid().await?;

        let mut container = Container::new(
            vmm_master_tid,
            config.clone(),
            &spec,
            self.agent.clone(),
            self.resource_manager.clone(),
        )
        .await
        .context("new container")?;

        let mut containers = self.containers.write().await;
        anyhow::ensure!(
            !containers.contains_key(&config.container_id),
            "container already registered"
        );
        self.oom_registry.register(&config.container_id).await;
        if let Err(e) = container.create(spec).await {
            self.oom_registry.remove(&config.container_id).await;
            if let Err(inner_e) = container.cleanup().await {
                warn!(sl!(), "failed to cleanup container {:?}", inner_e);
            }

            return Err(e);
        }

        containers.insert(container.container_id.to_string(), container);
        Ok(PID {
            pid: vmm_master_tid,
        })
    }

    #[instrument]
    async fn close_process_io(&self, process: &ContainerProcess) -> Result<()> {
        let containers = self.containers.read().await;
        let container_id = &process.container_id.to_string();
        let c = containers
            .get(container_id)
            .ok_or_else(|| Error::ContainerNotFound(container_id.clone()))?;

        c.close_io(process).await.context("close io")?;
        Ok(())
    }

    #[instrument]
    async fn delete_process(&self, process: &ContainerProcess) -> Result<ProcessStateInfo> {
        let container_id = &process.container_id.container_id;
        match process.process_type {
            ProcessType::Container => {
                let mut containers = self.containers.write().await;
                let c = containers
                    .remove(container_id)
                    .ok_or_else(|| Error::ContainerNotFound(container_id.to_string()))?;

                self.oom_registry.remove(container_id).await;

                c.state_process(process).await.context("state process")
            }
            ProcessType::Exec => {
                let containers = self.containers.read().await;
                let c = containers
                    .get(container_id)
                    .ok_or_else(|| Error::ContainerNotFound(container_id.to_string()))?;
                let state = c.state_process(process).await.context("state process");
                c.delete_exec_process(process)
                    .await
                    .context("delete process")?;
                return state;
            }
        }
    }

    #[instrument]
    async fn exec_process(&self, req: ExecProcessRequest) -> Result<()> {
        if req.spec_type_url.is_empty() {
            return Err(anyhow!("invalid type url"));
        }
        let mut oci_process: OCIProcess =
            serde_json::from_slice(&req.spec_value).context("serde from slice")?;

        oci_process.set_apparmor_profile(None);
        oci_process.set_capabilities(None);

        // CRI-O derives an exec's process from the container's, so a container
        // declaring tty: true asks for a terminal the exec never wanted. Only
        // req.terminal says how the streams are copied, so it decides here too.
        oci_process.set_terminal(Some(req.terminal));

        let containers = self.containers.read().await;
        let container_id = &req.process.container_id.container_id;
        let c = containers
            .get(container_id)
            .ok_or_else(|| Error::ContainerNotFound(container_id.clone()))?;
        c.exec_process(
            &req.process,
            req.stdin,
            req.stdout,
            req.stderr,
            req.terminal,
            oci_process,
        )
        .await
        .context("container exec")
    }

    #[instrument]
    async fn kill_process(&self, req: &KillRequest) -> Result<()> {
        let containers = self.containers.read().await;
        let container_id = &req.process.container_id.container_id;

        // According to CRI specs, kubelet will call StopPodSandbox()
        // at least once before calling RemovePodSandbox and this call
        // is idempotent. It must not return an error if all relevant
        // resources have already been reclaimed
        let c = match containers.get(container_id) {
            Some(c) => c,
            None => {
                // Container already removed - this is OK for SIGKILL/SIGTERM
                if is_termination_signal(req.signal) {
                    warn!(
                        sl!(),
                        "Signal {} ignored due to container not existing", req.signal;
                        "container" => container_id,
                        "signal" => req.signal
                    );
                    return Ok(());
                }
                return Err(Error::ContainerNotFound(container_id.clone()).into());
            }
        };

        // According to CRI specs, kubelet will call StopPodSandbox()
        // at least once before calling RemovePodSandbox, and this call
        // is idempotent, and must not return an error if all relevant
        // resources have already been reclaimed. And in that call it will
        // send a SIGKILL signal first to try to stop the container, thus
        // once the container has terminated, here should ignore this signal
        // and return directly.
        //
        // When the VM/agent is dead (e.g., QEMU killed externally), the ttrpc
        // connection will fail with AgentConnectionClosed error.
        // Additionally, if the container's init process is already gone, the
        // agent returns ProcessAlreadyTerminated error.
        // For SIGKILL/SIGTERM, we should treat these as success since the
        // container is effectively terminated.
        c.kill_process(&req.process, req.signal, req.all)
            .await
            .or_else(|err| {
                let is_term_signal = is_termination_signal(req.signal);

                // Check for typed errors using downcast_ref
                let is_expected_error = matches!(
                    err.downcast_ref::<Error>(),
                    Some(Error::AgentConnectionClosed) | Some(Error::ProcessAlreadyTerminated)
                );

                if is_term_signal && is_expected_error {
                    warn!(
                        sl!(),
                        "Signal encounters expected error, VM/process already terminated";
                        "container" => container_id,
                        "process" => ?&req.process,
                        "signal" => req.signal,
                    );
                    Ok(())
                } else {
                    Err(err)
                }
            })
    }

    #[instrument]
    async fn wait_process(&self, process: &ContainerProcess) -> Result<ProcessExitStatus> {
        let logger = logger_with_process(process);

        let containers = self.containers.read().await;
        let container_id = &process.container_id.container_id;
        let c = containers
            .get(container_id)
            .ok_or_else(|| Error::ContainerNotFound(container_id.clone()))?;
        let (mut watcher, status) = c.wait_process(process).await.context("wait")?;
        drop(containers);

        info!(logger, "begin wait exit");
        while watcher.changed().await.is_ok() {}
        info!(logger, "end wait exited");

        let status = status.read().await;

        info!(logger, "wait process exit status {:?}", status);

        Ok(status.clone())
    }

    #[instrument]
    async fn start_process(&self, process: &ContainerProcess) -> Result<PID> {
        let containers = self.containers.read().await;
        let container_id = &process.container_id.container_id;
        let c = containers
            .get(container_id)
            .ok_or_else(|| Error::ContainerNotFound(container_id.clone()))?;
        c.start(self.containers.clone(), process)
            .await
            .context("start")?;

        let vmm_master_tid = self.get_vmm_master_tid().await?;
        Ok(PID {
            pid: vmm_master_tid,
        })
    }

    #[instrument]
    async fn state_process(&self, process: &ContainerProcess) -> Result<ProcessStateInfo> {
        let containers = self.containers.read().await;
        let container_id = &process.container_id.container_id;

        // When using Sandbox API, the sandbox container (container_id == sandbox_id)
        // is not stored in the containers map. Return a synthetic state for it.
        if let Some(c) = containers.get(container_id) {
            c.state_process(process).await.context("state process")
        } else if container_id == &self.sid {
            let vmm_pid = self.get_vmm_master_tid().await?;
            Ok(ProcessStateInfo {
                container_id: self.sid.clone(),
                exec_id: String::new(),
                pid: PID { pid: vmm_pid },
                bundle: String::new(),
                stdin: None,
                stdout: None,
                stderr: None,
                terminal: false,
                status: ProcessStatus::Running,
                exit_status: 0,
                exited_at: None,
            })
        } else {
            Err(Error::ContainerNotFound(container_id.clone()).into())
        }
    }

    #[instrument]
    async fn pause_container(&self, id: &ContainerID) -> Result<()> {
        let containers = self.containers.read().await;
        let c = containers
            .get(&id.container_id)
            .ok_or_else(|| Error::ContainerNotFound(id.container_id.clone()))?;
        c.pause().await.context("pause")?;
        Ok(())
    }

    #[instrument]
    async fn resume_container(&self, id: &ContainerID) -> Result<()> {
        let containers = self.containers.read().await;
        let c = containers
            .get(&id.container_id)
            .ok_or_else(|| Error::ContainerNotFound(id.container_id.clone()))?;
        c.resume().await.context("resume")?;
        Ok(())
    }

    #[instrument]
    async fn resize_process_pty(&self, req: &ResizePTYRequest) -> Result<()> {
        let containers = self.containers.read().await;
        let c = containers
            .get(&req.process.container_id.container_id)
            .ok_or_else(|| {
                Error::ContainerNotFound(req.process.container_id.container_id.clone())
            })?;
        c.resize_pty(&req.process, req.width, req.height)
            .await
            .context("resize pty")?;
        Ok(())
    }

    #[instrument]
    async fn stats_container(&self, id: &ContainerID) -> Result<StatsInfo> {
        let containers = self.containers.read().await;
        let c = containers
            .get(&id.container_id)
            .ok_or_else(|| Error::ContainerNotFound(id.container_id.clone()))?;
        let stats = c.stats().await.context("stats")?;
        Ok(StatsInfo::from(stats))
    }

    #[instrument]
    async fn update_container(&self, req: UpdateRequest) -> Result<()> {
        let resource = serde_json::from_slice::<oci::LinuxResources>(&req.value)
            .context("deserialize LinuxResource")?;
        let containers = self.containers.read().await;
        let container_id = &req.container_id;
        let c = containers
            .get(container_id)
            .ok_or_else(|| Error::ContainerNotFound(container_id.to_string()))?;
        c.update(&resource).await.context("update_container")
    }

    #[instrument]
    async fn pid(&self) -> Result<PID> {
        let vmm_pid = self.get_vmm_master_tid().await?;
        Ok(PID { pid: vmm_pid })
    }

    #[instrument]
    async fn connect_container(&self, _id: &ContainerID) -> Result<PID> {
        let vmm_pid = self.get_vmm_master_tid().await?;
        Ok(PID { pid: vmm_pid })
    }

    #[instrument]
    async fn need_shutdown_sandbox(&self, req: &ShutdownRequest) -> bool {
        req.is_now || self.sid == req.container_id
    }

    #[instrument]
    async fn is_sandbox_container(&self, process: &ContainerProcess) -> bool {
        process.process_type == ProcessType::Container
            && process.container_id.container_id == self.sid
    }
}

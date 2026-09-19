// Copyright (c) 2019-2022 Alibaba Cloud
// Copyright (c) 2019-2022 Ant Group
//
// SPDX-License-Identifier: Apache-2.0
//

use anyhow::{anyhow, Context, Result};
use common::{
    message::{Action, Message},
    types::{
        ContainerProcess, PlatformInfo, ProcessType, SandboxConfig, SandboxRequest,
        SandboxResponse, SandboxStatusInfo, StartSandboxInfo, TaskRequest, TaskResponse,
        DEFAULT_SHM_SIZE,
    },
    RuntimeInstance, Sandbox, SandboxNetworkEnv,
};

use containerd_shim_protos::events::task::{TaskCreate, TaskDelete, TaskStart};
use hypervisor::Param;
use kata_sys_util::{mount::get_mount_path, spec::load_oci_spec};
use kata_types::{
    annotations::Annotation,
    config::{default::DEFAULT_GUEST_DNS_FILE, TomlConfig},
    mount::SHM_DEVICE,
};

use logging::FILTER_RULE;
use netns_rs::NetNs;
use nix::sys::statfs;
use oci_spec::runtime as oci;
use persist::sandbox_persist::Persist;
use protobuf::Message as ProtobufMessage;
use resource::{
    cpu_mem::initial_size::InitialSizeManager,
    network::{generate_netns_name, reject_dan},
};
use runtime_spec as spec;
use shim_interface::shim_mgmt::ERR_NO_SHIM_SERVER;
use std::{
    collections::HashMap,
    ops::Deref,
    path::{Path, PathBuf},
    sync::Arc,
    time::SystemTime,
};
use tokio::fs;
use tokio::sync::{mpsc::Sender, Mutex, RwLock};
use tracing::instrument;
use virt_container::{
    sandbox::{SandboxRestoreArgs, VirtSandbox},
    sandbox_persist::SandboxState,
    VirtContainer,
};

use crate::{
    shim_mgmt::server::MgmtServer,
    tracer::{KataTracer, ROOTSPAN},
};

fn convert_string_to_slog_level(string_level: &str) -> slog::Level {
    match string_level {
        "trace" => slog::Level::Trace,
        "debug" => slog::Level::Debug,
        "info" => slog::Level::Info,
        "warn" => slog::Level::Warning,
        "error" => slog::Level::Error,
        "critical" => slog::Level::Critical,
        _ => slog::Level::Info,
    }
}

fn effective_log_level(enable_debug: bool, log_level: &str) -> &str {
    if enable_debug && log_level == "info" {
        "debug"
    } else {
        log_level
    }
}

struct RuntimeHandlerManagerInner {
    id: String,
    msg_sender: Sender<Message>,
    kata_tracer: Arc<Mutex<KataTracer>>,
    runtime_instance: Option<Arc<RuntimeInstance>>,
}

impl std::fmt::Debug for RuntimeHandlerManagerInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuntimeHandlerManagerInner")
            .field("id", &self.id)
            .field("msg_sender", &self.msg_sender)
            .finish()
    }
}

impl RuntimeHandlerManagerInner {
    fn new(id: &str, msg_sender: Sender<Message>) -> Self {
        let tracer = KataTracer::new();
        Self {
            id: id.to_string(),
            msg_sender,
            kata_tracer: Arc::new(Mutex::new(tracer)),
            runtime_instance: None,
        }
    }

    #[instrument]
    async fn init_runtime_handler(
        &mut self,
        sandbox_config: SandboxConfig,
        config: Arc<TomlConfig>,
    ) -> Result<()> {
        info!(sl!(), "new runtime handler {}", &config.runtime.name);
        let runtime_instance = VirtContainer::new_instance(
            &self.id,
            self.msg_sender.clone(),
            config.clone(),
            sandbox_config,
        )
        .await
        .context("new runtime instance")?;

        // initilize the trace subscriber
        if config.runtime.enable_tracing {
            let mut tracer = self.kata_tracer.lock().await;
            if let Err(e) = tracer.trace_setup(
                &self.id,
                &config.runtime.jaeger_endpoint,
                &config.runtime.jaeger_user,
                &config.runtime.jaeger_password,
            ) {
                warn!(sl!(), "failed to setup tracing, {:?}", e);
            }
        }

        let instance = Arc::new(runtime_instance);
        self.runtime_instance = Some(instance.clone());

        Ok(())
    }

    #[instrument]
    async fn try_init(
        &mut self,
        sandbox_config: SandboxConfig,
        spec: Option<&oci::Spec>,
        options: &Option<Vec<u8>>,
    ) -> Result<()> {
        VirtContainer::init().context("init virt container")?;

        let mut config =
            load_config(&sandbox_config.annotations, options).context("load config")?;

        let mut initial_size_manager = if let Some(spec) = spec {
            InitialSizeManager::new(spec).context("failed to construct static resource manager")?
        } else {
            InitialSizeManager::new_from(&sandbox_config.annotations)
                .context("failed to construct static resource manager")?
        };

        // For CRI sandboxes, sizing annotations are carried in PodSandboxConfig
        // and may be absent from the OCI sandbox spec. Fill any missing sizing
        // values from sandbox annotations before applying static sizing.
        initial_size_manager
            .supplement_from_annotations(&sandbox_config.annotations)
            .context("failed to supplement static resource manager from annotations")?;

        initial_size_manager
            .setup_config(&mut config)
            .context("failed to setup static resource mgmt config")?;

        update_component_log_level(&config);

        reject_dan(&config, &self.id)?;
        self.init_runtime_handler(sandbox_config, Arc::new(config))
            .await
            .context("init runtime handler")?;

        let shim_mgmt_svr = MgmtServer::new(
            &self.id,
            self.runtime_instance.as_ref().unwrap().sandbox.clone(),
        )
        .context(ERR_NO_SHIM_SERVER)?;

        tokio::task::spawn(Arc::new(shim_mgmt_svr).run());
        info!(sl!(), "shim management http server starts");

        Ok(())
    }

    fn get_runtime_instance(&self) -> Option<Arc<RuntimeInstance>> {
        self.runtime_instance.clone()
    }

    fn get_kata_tracer(&self) -> Arc<Mutex<KataTracer>> {
        self.kata_tracer.clone()
    }
}

pub struct RuntimeHandlerManager {
    inner: Arc<RwLock<RuntimeHandlerManagerInner>>,
}

// todo: a more detailed impl for fmt::Debug
impl std::fmt::Debug for RuntimeHandlerManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuntimeHandlerManager").finish()
    }
}

impl RuntimeHandlerManager {
    pub fn new(id: &str, msg_sender: Sender<Message>) -> Self {
        Self {
            inner: Arc::new(RwLock::new(RuntimeHandlerManagerInner::new(id, msg_sender))),
        }
    }

    pub async fn cleanup(&self) -> Result<()> {
        let inner = self.inner.read().await;
        let sender = inner.msg_sender.clone();
        let sandbox_state = persist::from_disk::<SandboxState>(&inner.id)
            .context("failed to load the sandbox state")?;

        let config = if let Ok(spec) = load_oci_spec() {
            let annotations = spec.annotations().clone().unwrap_or_default();
            load_config(&annotations, &None).context("load config")?
        } else {
            TomlConfig::default()
        };

        let sandbox_args = SandboxRestoreArgs {
            sid: inner.id.clone(),
            toml_config: config,
            sender,
        };
        match sandbox_state.sandbox_type.clone() {
            name if name == virt_container::sandbox::VIRTCONTAINER => {
                if sandbox_args.toml_config.runtime.keep_abnormal {
                    info!(sl!(), "skip cleanup for keep_abnormal");
                    return Ok(());
                }
                let sandbox = VirtSandbox::restore(sandbox_args, sandbox_state)
                    .await
                    .context("failed to restore the sandbox")?;
                sandbox
                    .cleanup()
                    .await
                    .context("failed to cleanup the resource")?;
            }
            _ => {
                return Ok(());
            }
        }

        Ok(())
    }

    async fn get_runtime_instance(&self) -> Result<Arc<RuntimeInstance>> {
        let inner = self.inner.read().await;
        inner
            .get_runtime_instance()
            .ok_or_else(|| anyhow!("runtime not ready"))
    }

    async fn get_kata_tracer(&self) -> Result<Arc<Mutex<KataTracer>>> {
        let inner = self.inner.read().await;
        Ok(inner.get_kata_tracer())
    }

    //init the sandbox for the normal task api
    #[instrument]
    async fn task_init_runtime_instance(
        &self,
        spec: &mut oci::Spec,
        options: &Option<Vec<u8>>,
    ) -> Result<()> {
        let mut inner: tokio::sync::RwLockWriteGuard<'_, RuntimeHandlerManagerInner> =
            self.inner.write().await;

        // return if runtime instance has init
        if inner.runtime_instance.is_some() {
            return Ok(());
        }

        let mut dns: Vec<String> = vec![];

        let spec_mounts = spec.mounts().clone().unwrap_or_default();
        for m in &spec_mounts {
            if get_mount_path(&Some(m.destination().clone())) == DEFAULT_GUEST_DNS_FILE {
                let contents = fs::read_to_string(&Path::new(&get_mount_path(m.source()))).await?;
                dns = contents.split('\n').map(|e| e.to_string()).collect();
            }
        }

        let mut network_created = false;
        let mut netns = None;
        if let Some(linux) = &spec.linux() {
            let linux_namespaces = linux.namespaces().clone().unwrap_or_default();
            for ns in &linux_namespaces {
                if ns.typ() != oci::LinuxNamespaceType::Network {
                    continue;
                }
                // get netns path from oci spec
                if ns.path().is_some() {
                    netns = ns.path().clone().map(|p| p.display().to_string());
                }
                // if we get empty netns from oci spec, we need to create netns for the VM
                else {
                    let ns_name = generate_netns_name();
                    let raw_netns = NetNs::new(ns_name)?;
                    let path = Some(PathBuf::from(raw_netns.path()).display().to_string());
                    netns = path;
                    network_created = true;
                }
                break;
            }
        }

        // When the OCI spec contains a network namespace with path `/proc/0/ns/net`,
        // it means the task PID was not yet known at spec generation time (PID 0 is a
        // placeholder).  containerd populates the netns path before the shim returns
        // a real PID via the Connect RPC.  Treat this as "no netns provided" so the
        // rescan mechanism can discover the correct namespace later.
        if netns.as_deref() == Some("/proc/0/ns/net") {
            netns = None;
        }
        // A nerdctl network namespace to let nerdctl know which namespace to use when calling the
        // selected CNI plugin.
        if let Some(netns_path) = &netns {
            if spec.annotations_mut().is_none() {
                spec.set_annotations(Some(HashMap::new()));
            }
            if let Some(annotations) = spec.annotations_mut().as_mut() {
                annotations.insert("nerdctl/network-namespace".to_string(), netns_path.clone());
            }
        }

        let network_env = SandboxNetworkEnv {
            netns,
            network_created,
        };

        let shm_size = get_shm_size(spec)?;

        let sandbox_config = SandboxConfig {
            sandbox_id: inner.id.clone(),
            dns,
            hostname: spec.hostname().clone().unwrap_or_default(),
            network_env,
            annotations: spec.annotations().clone().unwrap_or_default(),
            shm_size,
        };

        inner.try_init(sandbox_config, Some(spec), options).await
    }

    //init the sandbox for the sandbox api
    #[instrument]
    async fn sandbox_init_runtime_instance(&self, sandbox_config: SandboxConfig) -> Result<()> {
        let mut inner = self.inner.write().await;
        // return if runtime instance has init
        if inner.runtime_instance.is_some() {
            return Ok(());
        }
        inner.try_init(sandbox_config, None, &None).await
    }

    #[instrument(parent = &*(ROOTSPAN))]
    pub async fn handler_sandbox_message(&self, req: SandboxRequest) -> Result<SandboxResponse> {
        if let SandboxRequest::CreateSandbox(sandbox_config) = req {
            let config = sandbox_config.deref().clone();

            self.sandbox_init_runtime_instance(config)
                .await
                .context("init sandboxed runtime")?;

            Ok(SandboxResponse::CreateSandbox)
        } else {
            self.handler_sandbox_request(req)
                .await
                .context("handler request")
        }
    }

    #[instrument(parent = &*(ROOTSPAN))]
    pub async fn handler_task_message(&self, req: TaskRequest) -> Result<TaskResponse> {
        if let TaskRequest::CreateContainer(container_config) = req {
            // get oci spec
            let bundler_path = format!(
                "{}/{}",
                container_config.bundle,
                spec::OCI_SPEC_CONFIG_FILE_NAME
            );
            let mut spec = oci::Spec::load(&bundler_path).context("load spec")?;
            kata_types::device::validate_spec_device_features(&spec)?;
            self.task_init_runtime_instance(&mut spec, &container_config.options)
                .await
                .context("try init runtime instance")?;
            let instance = self
                .get_runtime_instance()
                .await
                .context("get runtime instance")?;

            instance
                .sandbox
                .start()
                .await
                .context("start sandbox in task handler")?;

            let bundle = container_config.bundle.clone();
            let container_id = container_config.container_id.clone();
            let shim_pid = instance
                .container_manager
                .create_container(container_config, spec)
                .await
                .context("create container")?;

            let container_manager = instance.container_manager.clone();
            let process_id =
                ContainerProcess::new(&container_id, "").context("create container process")?;
            let pid = shim_pid.pid;
            tokio::spawn(async move {
                let result = instance
                    .sandbox
                    .wait_process(container_manager, process_id, pid)
                    .await;
                if let Err(e) = result {
                    error!(sl!(), "sandbox wait process error: {:?}", e);
                }
            });

            let msg_sender = self.inner.read().await.msg_sender.clone();
            let event = TaskCreate {
                container_id,
                bundle,
                pid,
                ..Default::default()
            };
            let msg = Message::new(Action::Event(Arc::new(event)));
            msg_sender
                .send(msg)
                .await
                .context("send task create event")?;

            Ok(TaskResponse::CreateContainer(shim_pid))
        } else {
            // A teardown RPC must still make the shim daemon exit even when
            // the runtime instance was never (fully) created -- e.g. after a
            // failed CreateContainer.  In that case containerd's follow-up
            // Shutdown would otherwise hit `get_runtime_instance()`, fail with
            // "runtime not ready", and the service loop would never receive
            // `Action::Shutdown`.  Because the shim ignores SIGTERM the daemon
            // would then be left running and orphaned by containerd.
            if let TaskRequest::ShutdownContainer(_) = &req {
                if self.get_runtime_instance().await.is_err() {
                    warn!(
                        sl!(),
                        "shutdown requested but runtime instance is not ready; \
                         forcing shim exit to avoid an orphaned shim process"
                    );
                    let sender = self.inner.read().await.msg_sender.clone();
                    sender
                        .send(Message::new(Action::Shutdown))
                        .await
                        .context("send shutdown message")?;
                    return Ok(TaskResponse::ShutdownContainer);
                }
            }

            self.handler_task_request(req)
                .await
                .context("handler TaskRequest")
        }
    }

    pub async fn handler_sandbox_request(&self, req: SandboxRequest) -> Result<SandboxResponse> {
        let instance = self
            .get_runtime_instance()
            .await
            .context("get runtime instance")?;
        let sandbox = instance.sandbox.clone();

        match req {
            SandboxRequest::CreateSandbox(req) => Err(anyhow!("Unreachable request {:?}", req)),
            SandboxRequest::StartSandbox(_) => {
                sandbox
                    .start()
                    .await
                    .context("start sandbox in sandbox handler")?;
                Ok(SandboxResponse::StartSandbox(StartSandboxInfo {
                    pid: std::process::id(),
                    create_time: Some(SystemTime::now()),
                }))
            }
            SandboxRequest::Platform(_) => Ok(SandboxResponse::Platform(PlatformInfo {
                os: std::env::consts::OS.to_string(),
                architecture: std::env::consts::ARCH.to_string(),
            })),
            SandboxRequest::StopSandbox(_) => {
                sandbox.stop().await.context("stop sandbox")?;

                Ok(SandboxResponse::StopSandbox)
            }
            SandboxRequest::WaitSandbox(_) => {
                let exit_info = sandbox.wait().await.context("wait sandbox")?;

                Ok(SandboxResponse::WaitSandbox(exit_info))
            }
            SandboxRequest::SandboxStatus(_) => {
                let status = sandbox.status().await?;

                Ok(SandboxResponse::SandboxStatus(SandboxStatusInfo {
                    sandbox_id: status.sandbox_id,
                    pid: status.pid,
                    state: status.state,
                    created_at: status.created_at,
                    exited_at: None,
                }))
            }
            SandboxRequest::Ping(_) => Ok(SandboxResponse::Ping),
            SandboxRequest::ShutdownSandbox(_) => {
                sandbox.shutdown().await.context("shutdown sandbox")?;

                Ok(SandboxResponse::ShutdownSandbox)
            }
        }
    }

    #[instrument(parent = &(*ROOTSPAN))]
    pub async fn handler_task_request(&self, req: TaskRequest) -> Result<TaskResponse> {
        let instance = self
            .get_runtime_instance()
            .await
            .context("get runtime instance")?;
        let sandbox = instance.sandbox.clone();
        let cm = instance.container_manager.clone();
        let msg_sender = self.inner.read().await.msg_sender.clone();

        match req {
            TaskRequest::CreateContainer(req) => Err(anyhow!("Unreachable TaskRequest {:?}", req)),
            TaskRequest::CloseProcessIO(process_id) => {
                cm.close_process_io(&process_id).await.context("close io")?;
                Ok(TaskResponse::CloseProcessIO)
            }
            TaskRequest::DeleteProcess(process_id) => {
                let resp = cm.delete_process(&process_id).await.context("do delete")?;
                if process_id.process_type == ProcessType::Container {
                    let event = TaskDelete {
                        id: process_id.container_id().to_string(),
                        pid: resp.pid.pid,
                        exit_status: resp.exit_status as u32,
                        ..Default::default()
                    };
                    let msg = Message::new(Action::Event(Arc::new(event)));
                    msg_sender
                        .send(msg)
                        .await
                        .context("send task delete event")?;
                }

                Ok(TaskResponse::DeleteProcess(resp))
            }
            TaskRequest::ExecProcess(req) => {
                cm.exec_process(req).await.context("exec")?;
                Ok(TaskResponse::ExecProcess)
            }
            TaskRequest::KillProcess(req) => {
                cm.kill_process(&req).await.context("kill process")?;
                Ok(TaskResponse::KillProcess)
            }
            TaskRequest::ShutdownContainer(req) => {
                if cm.need_shutdown_sandbox(&req).await {
                    sandbox.shutdown().await.context("do shutdown")?;

                    // stop the tracer collector
                    let kata_tracer = self.get_kata_tracer().await.context("get kata tracer")?;
                    let tracer = kata_tracer.lock().await;
                    tracer.trace_end();
                }
                Ok(TaskResponse::ShutdownContainer)
            }
            TaskRequest::WaitProcess(process_id) => {
                let exit_status = cm.wait_process(&process_id).await.context("wait process")?;
                if cm.is_sandbox_container(&process_id).await {
                    sandbox.stop().await.context("stop sandbox")?;

                    // Release sandbox resources (cgroup, network, mounts, ...)
                    // as soon as the sandbox container exits instead of waiting
                    // for an explicit ShutdownContainer/Delete RPC.  Engines
                    // like Docker only send those when the container is removed
                    // (e.g. with `--rm`); without this the sandbox cgroup would
                    // leak and collide with the next run.
                    sandbox.cleanup().await.context("cleanup sandbox")?;
                }
                Ok(TaskResponse::WaitProcess(exit_status))
            }
            TaskRequest::StartProcess(process_id) => {
                let shim_pid = cm
                    .start_process(&process_id)
                    .await
                    .context("start process")?;

                let pid = shim_pid.pid;
                let process_type = process_id.process_type;
                let container_id = process_id.container_id().to_string();
                tokio::spawn(async move {
                    let result = sandbox.wait_process(cm, process_id, pid).await;
                    if let Err(e) = result {
                        error!(sl!(), "sandbox wait process error: {:?}", e);
                    }
                });

                if process_type == ProcessType::Container {
                    let event = TaskStart {
                        container_id,
                        pid,
                        ..Default::default()
                    };
                    let msg = Message::new(Action::Event(Arc::new(event)));
                    msg_sender
                        .send(msg)
                        .await
                        .context("send task start event")?;
                }

                Ok(TaskResponse::StartProcess(shim_pid))
            }

            TaskRequest::StateProcess(process_id) => {
                let state = cm
                    .state_process(&process_id)
                    .await
                    .context("state process")?;
                Ok(TaskResponse::StateProcess(state))
            }
            TaskRequest::PauseContainer(container_id) => {
                cm.pause_container(&container_id)
                    .await
                    .context("pause container")?;
                Ok(TaskResponse::PauseContainer)
            }
            TaskRequest::ResumeContainer(container_id) => {
                cm.resume_container(&container_id)
                    .await
                    .context("resume container")?;
                Ok(TaskResponse::ResumeContainer)
            }
            TaskRequest::ResizeProcessPTY(req) => {
                cm.resize_process_pty(&req).await.context("resize pty")?;
                Ok(TaskResponse::ResizeProcessPTY)
            }
            TaskRequest::StatsContainer(container_id) => {
                let stats = cm
                    .stats_container(&container_id)
                    .await
                    .context("stats container")?;
                Ok(TaskResponse::StatsContainer(stats))
            }
            TaskRequest::UpdateContainer(req) => {
                cm.update_container(req).await.context("update container")?;
                Ok(TaskResponse::UpdateContainer)
            }
            TaskRequest::Pid => Ok(TaskResponse::Pid(cm.pid().await.context("pid")?)),
            TaskRequest::ConnectContainer(container_id) => Ok(TaskResponse::ConnectContainer(
                cm.connect_container(&container_id)
                    .await
                    .context("connect")?,
            )),
        }
    }
}

/// Config override ordering (highest first): shipped environment path,
/// containerd shim options, then default config files.
#[instrument]
fn load_config(an: &HashMap<String, String>, option: &Option<Vec<u8>>) -> Result<TomlConfig> {
    const KATA_CONF_FILE: &str = "KATA_CONF_FILE";
    virt_container::contract::validate_annotations(an)?;
    let annotation = Annotation::new(an.clone());
    // Clone a logger from global logger to ensure the logs in this function get flushed when drop
    let logger = slog::Logger::clone(&slog_scope::logger());

    let config_path = if let Ok(path) = std::env::var(KATA_CONF_FILE) {
        if is_shipped_kata_config_path(&path) {
            path
        } else {
            return Err(anyhow!(
                "invalid KATA_CONF_FILE {:?}: only shipped Kata configuration files are accepted",
                path
            ));
        }
    } else if let Some(option) = option {
        // Parse the containerd runtime options protobuf message to extract the config path.
        // The options are passed as a serialized runtimeoptions.v1.Options protobuf message
        // from containerd's configuration (e.g., [plugins."io.containerd.grpc.v1.cri".containerd.runtimes.kata.options]).
        match <protocols::runtimeoptions::Options as ProtobufMessage>::parse_from_bytes(option) {
            Ok(opts) => opts.config_path,
            Err(e) => {
                // Log the error but don't fail - fall back to default config paths
                let logger = slog::Logger::clone(&slog_scope::logger());
                slog::warn!(
                    logger,
                    "failed to parse containerd runtime options: {}, falling back to default config paths",
                    e
                );
                String::from("")
            }
        }
    } else {
        String::from("")
    };

    info!(logger, "get config path {:?}", &config_path);
    let (mut toml_config, _) = TomlConfig::load_from_file(&config_path).context(format!(
        "load TOML config failed (tried {:?})",
        TomlConfig::get_default_config_file_list()
    ))?;
    annotation.update_config_by_annotation(&mut toml_config)?;
    update_agent_kernel_params(&mut toml_config)?;

    // validate configuration and return the error
    toml_config.validate()?;
    virt_container::contract::validate(&toml_config)?;

    info!(logger, "get config content {:?}", &toml_config);
    Ok(toml_config)
}

fn is_shipped_kata_config_path(config_path: &str) -> bool {
    config_path_matches_defaults(config_path, TomlConfig::get_default_config_file_list())
}

fn config_path_matches_defaults(config_path: &str, default_config_paths: Vec<PathBuf>) -> bool {
    let Ok(resolved_config_path) = std::fs::canonicalize(config_path) else {
        return false;
    };

    default_config_paths
        .into_iter()
        .filter_map(|path| std::fs::canonicalize(path).ok())
        .any(|path| path == resolved_config_path)
}

// this update the agent-specfic kernel parameters into hypervisor's bootinfo
// the agent inside the VM will read from file cmdline to get the params and function
fn update_agent_kernel_params(config: &mut TomlConfig) -> Result<()> {
    let mut params = vec![];
    if let Ok(kv) = config.get_agent_kernel_params() {
        for (k, v) in kv.into_iter() {
            if let Ok(s) = Param::new(k.as_str(), v.as_str()).to_string() {
                params.push(s);
            }
        }
        if let Some(h) = config.hypervisor.get_mut(&config.runtime.hypervisor_name) {
            h.boot_info.add_kernel_params(params);
        }
    }
    Ok(())
}

// this update the log_level of three component: agent, hypervisor, runtime
// according to the settings read from configuration file
fn update_component_log_level(config: &TomlConfig) {
    // Retrieve the log-levels set in configuration file, modify the FILTER_RULE accordingly
    let default_level = "info";
    let agent_level = if let Some(agent_config) = config.agent.get(&config.runtime.agent_name) {
        effective_log_level(agent_config.debug, &agent_config.log_level)
    } else {
        default_level
    };
    let hypervisor_level =
        if let Some(hypervisor_config) = config.hypervisor.get(&config.runtime.hypervisor_name) {
            effective_log_level(
                hypervisor_config.debug_info.enable_debug,
                &hypervisor_config.debug_info.log_level,
            )
        } else {
            default_level
        };
    let runtime_level = effective_log_level(config.runtime.debug, &config.runtime.log_level);

    // Update FILTER_RULE to apply changes
    FILTER_RULE.rcu(|inner| {
        let mut updated_inner = HashMap::new();
        updated_inner.clone_from(inner);
        updated_inner.insert(
            "runtimes".to_string(),
            convert_string_to_slog_level(runtime_level),
        );
        updated_inner.insert(
            "agent".to_string(),
            convert_string_to_slog_level(agent_level),
        );
        updated_inner.insert(
            "hypervisor".to_string(),
            convert_string_to_slog_level(hypervisor_level),
        );
        updated_inner
    });
}

fn get_shm_size(spec: &oci::Spec) -> Result<u64> {
    let mut shm_size = DEFAULT_SHM_SIZE;

    if let Some(mounts) = spec.mounts() {
        for m in mounts {
            if m.destination().as_path() != Path::new(SHM_DEVICE) {
                continue;
            }

            if m.typ().eq(&Some("bind".to_string()))
                && !m.source().eq(&Some(PathBuf::from(SHM_DEVICE)))
            {
                if let Some(src) = m.source() {
                    let statfs = statfs::statfs(src)?;
                    shm_size = statfs.blocks() * statfs.block_size() as u64;
                }
            }
        }
    }

    Ok(shm_size)
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::types::ShutdownRequest;
    use rstest::rstest;
    use tokio::sync::mpsc::channel;

    // A ShutdownContainer RPC that arrives before any runtime instance was
    // created (e.g. after a failed CreateContainer) must still drive the shim
    // daemon to exit, otherwise the process is orphaned.  Verify it returns
    // ShutdownContainer and emits Action::Shutdown on the service channel.
    #[tokio::test]
    async fn test_shutdown_without_runtime_instance_forces_exit() {
        let (sender, mut receiver) = channel::<Message>(8);
        let manager = RuntimeHandlerManager::new("test-sid", sender);

        let resp = manager
            .handler_task_message(TaskRequest::ShutdownContainer(ShutdownRequest {
                container_id: "test-sid".to_string(),
                is_now: true,
            }))
            .await
            .expect("shutdown should succeed even without a runtime instance");

        assert!(matches!(resp, TaskResponse::ShutdownContainer));

        let msg = receiver
            .try_recv()
            .expect("an Action::Shutdown message must be sent to stop the daemon");
        assert!(matches!(msg.action, Action::Shutdown));
    }

    #[test]
    fn test_effective_log_level() {
        assert_eq!(effective_log_level(false, "info"), "info");
        assert_eq!(effective_log_level(false, "debug"), "debug");
        assert_eq!(effective_log_level(true, "info"), "debug");
        assert_eq!(effective_log_level(true, "trace"), "trace");
        assert_eq!(effective_log_level(true, "warn"), "warn");
    }

    #[derive(Debug)]
    enum ConfigPathCase {
        Shipped,
        NonShipped,
        NonExistent,
        Empty,
    }

    #[rstest]
    #[case::shipped_config_is_accepted(ConfigPathCase::Shipped, true)]
    #[case::non_shipped_config_is_rejected(ConfigPathCase::NonShipped, false)]
    #[case::non_existent_path_is_rejected(ConfigPathCase::NonExistent, false)]
    #[case::empty_path_is_rejected(ConfigPathCase::Empty, false)]
    fn test_config_path_matches_defaults(
        #[case] path_case: ConfigPathCase,
        #[case] expected: bool,
    ) {
        let tmpdir = tempfile::tempdir().unwrap();
        let shipped_path = tmpdir.path().join("shipped.toml");
        let non_shipped_path = tmpdir.path().join("malicious.toml");
        std::fs::write(&shipped_path, b"[hypervisor.qemu]\n").unwrap();
        std::fs::write(&non_shipped_path, b"[hypervisor.qemu]\n").unwrap();

        // Only the shipped path is treated as a default config location.
        let default_config_paths = vec![shipped_path.clone()];

        let config_path = match path_case {
            ConfigPathCase::Shipped => shipped_path.to_string_lossy().to_string(),
            ConfigPathCase::NonShipped => non_shipped_path.to_string_lossy().to_string(),
            ConfigPathCase::NonExistent => tmpdir
                .path()
                .join("nonexistent.toml")
                .to_string_lossy()
                .to_string(),
            ConfigPathCase::Empty => String::new(),
        };

        assert_eq!(
            config_path_matches_defaults(&config_path, default_config_paths),
            expected,
            "case {:?}: unexpected result for path {:?}",
            path_case,
            config_path,
        );
    }
}

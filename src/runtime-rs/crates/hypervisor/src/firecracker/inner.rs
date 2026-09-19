//Copyright (c) 2019-2022 Alibaba Cloud
//Copyright (c) 2023 Nubificus Ltd
//
//SPDX-License-Identifier: Apache-2.0

use crate::firecracker::{inner_hypervisor::FC_API_SOCKET_NAME, sl};
use crate::HYPERVISOR_FIRECRACKER;
use crate::{device::DeviceType, VmmState};
use crate::{selinux, HypervisorState};
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use bytes::Bytes;
use http_body_util::Full;
use hyper_util::client::legacy::Client;
use hyperlocal::{UnixClientExt, UnixConnector};
use kata_sys_util::guest_io::{read_line, RateLimit};
use kata_types::{
    capabilities::{Capabilities, CapabilityBits},
    config::hypervisor::Hypervisor as HypervisorConfig,
};
use nix::sched::{setns, CloneFlags};
use persist::sandbox_persist::Persist;
use std::process::Stdio;
use tokio::io::BufReader;
use tokio::process::{Child, ChildStderr, Command};
use tokio::sync::mpsc;
use tokio::sync::Mutex;

unsafe impl Send for FcInner {}
unsafe impl Sync for FcInner {}

#[derive(Debug)]
pub struct FcInner {
    pub(crate) id: String,
    pub(crate) asock_path: String,
    pub(crate) state: VmmState,
    pub(crate) config: HypervisorConfig,
    pub(crate) pid: Option<u32>,
    pub(crate) vm_path: String,
    pub(crate) netns: Option<String>,
    pub(crate) client: Client<UnixConnector, Full<Bytes>>,
    pub(crate) jailer_root: String,
    pub(crate) run_dir: String,
    pub(crate) pending_devices: Vec<DeviceType>,
    pub(crate) capabilities: Capabilities,
    pub(crate) fc_process: Mutex<Option<Child>>,
    pub(crate) exit_notify: Option<mpsc::Sender<()>>,
}

impl FcInner {
    pub fn new(exit_notify: mpsc::Sender<()>) -> FcInner {
        let mut capabilities = Capabilities::new();
        capabilities.set(CapabilityBits::BlockDeviceSupport | CapabilityBits::HybridVsockSupport);

        FcInner {
            id: String::default(),
            asock_path: String::default(),
            state: VmmState::NotReady,
            config: Default::default(),
            pid: None,
            netns: None,
            vm_path: String::default(),
            client: Client::unix(),
            jailer_root: String::default(),
            run_dir: String::default(),
            pending_devices: vec![],
            capabilities,
            fc_process: Mutex::new(None),
            exit_notify: Some(exit_notify),
        }
    }

    pub(crate) async fn prepare_vmm(&mut self, netns: Option<String>) -> Result<()> {
        validate_jailer_config(&self.config)?;
        self.netns = netns.clone();
        let mut cmd = Command::new(&self.config.jailer_path);
        let api_socket = format!("/run/{FC_API_SOCKET_NAME}");
        cmd.args([
            "--id",
            &self.id,
            "--gid",
            "0",
            "--uid",
            "0",
            "--exec-file",
            &self.config.path,
            "--chroot-base-dir",
            &self.jailer_root,
            "--",
            "--api-sock",
            &api_socket,
        ]);
        debug!(sl(), "Exec: {:?}", cmd);

        // Make sure we're in the correct Network Namespace
        unsafe {
            let selinux_label = self.config.security_info.selinux_label.clone();
            let _pre = cmd.pre_exec(move || {
                if let Some(netns_path) = &netns {
                    debug!(sl(), "set netns for vmm master {:?}", &netns_path);
                    let netns_fd = std::fs::File::open(netns_path);
                    let _ = setns(&netns_fd?, CloneFlags::CLONE_NEWNET).context("set netns failed");
                }
                if let Some(label) = selinux_label.as_ref() {
                    if let Err(e) = selinux::set_exec_label(label) {
                        error!(sl!(), "Failed to set SELinux label in child process: {}", e);
                        // Don't return error here to avoid breaking the process startup
                        // Log the error and continue
                    } else {
                        info!(
                            sl!(),
                            "Successfully set SELinux label in child process: {}", &label
                        );
                    }
                }
                Ok(())
            });
        }

        let mut child = cmd.stderr(Stdio::piped()).spawn()?;

        let stderr = child.stderr.take().unwrap();
        let exit_notify = self
            .exit_notify
            .take()
            .ok_or_else(|| anyhow!("no exit notify"))?;
        tokio::spawn(log_fc_stderr(stderr, exit_notify));

        match child.id() {
            Some(id) => {
                let cur_tid = nix::unistd::gettid().as_raw() as u32;
                info!(
                    sl(),
                    "VMM spawned successfully: PID: {:?}, current TID: {:?}", id, cur_tid
                );
                self.pid = Some(id);
            }
            None => {
                let exit_status = child.wait().await?;
                error!(sl(), "Process exited, status: {:?}", exit_status);
                return Err(anyhow!("fc vmm start failed with: {:?}", exit_status));
            }
        };

        self.fc_process = Mutex::new(Some(child));

        Ok(())
    }

    pub(crate) fn hypervisor_config(&self) -> HypervisorConfig {
        debug!(sl(), "[Firecracker]: Hypervisor config");
        self.config.clone()
    }

    pub(crate) fn set_hypervisor_config(&mut self, config: HypervisorConfig) {
        debug!(sl(), "[Firecracker]: Set Hypervisor config");
        self.config = config;
    }
}

pub(super) fn validate_jailer_config(config: &HypervisorConfig) -> Result<()> {
    anyhow::ensure!(
        !config.jailer_path.is_empty(),
        "kata-fc: Firecracker jailer is required"
    );
    anyhow::ensure!(
        !config.security_info.disable_seccomp,
        "kata-fc: Firecracker seccomp cannot be disabled"
    );
    Ok(())
}

async fn log_fc_stderr(stderr: ChildStderr, exit_notify: mpsc::Sender<()>) -> Result<()> {
    info!(sl!(), "starting reading fc stderr");

    let mut stderr_reader = BufReader::new(stderr);
    let mut budget = RateLimit::new(1024, 1024 * 1024);
    let result: std::io::Result<()> = async {
        while let Some(buffer) = read_line(&mut stderr_reader, 64 * 1024).await? {
            budget.check(buffer.len())?;
            info!(sl!(), "fc stderr: {:?}", buffer);
        }
        Ok(())
    }
    .await;
    if let Err(error) = result {
        warn!(
            sl!(),
            "VMM logging disabled for invalid/excessive input: {}", error
        );
        // Keep the pipe open and drain with bounded memory and throughput. Logging errors
        // must not deliver SIGPIPE to the VMM or masquerade as its process exit.
        use tokio::io::AsyncReadExt;
        let mut discard = [0u8; 8192];
        while let Ok(count) = stderr_reader.read(&mut discard).await {
            if count == 0 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }

    // Notfiy the waiter the process exit.
    let _ = exit_notify.try_send(());

    info!(sl!(), "finished reading fc stderr");
    Ok(())
}

#[async_trait]
impl Persist for FcInner {
    type State = HypervisorState;
    type ConstructorArgs = mpsc::Sender<()>;

    async fn save(&self) -> Result<Self::State> {
        Ok(HypervisorState {
            hypervisor_type: HYPERVISOR_FIRECRACKER.to_string(),
            id: self.id.clone(),
            vm_path: self.vm_path.clone(),
            config: self.hypervisor_config(),
            jailed: true,
            jailer_root: self.jailer_root.clone(),
            run_dir: self.run_dir.clone(),
            netns: self.netns.clone(),
            ..Default::default()
        })
    }
    async fn restore(exit_notify: mpsc::Sender<()>, hypervisor_state: Self::State) -> Result<Self> {
        anyhow::ensure!(
            hypervisor_state.jailed,
            "kata-fc: unjailed saved VMs are unsupported"
        );
        validate_jailer_config(&hypervisor_state.config)?;
        Ok(FcInner {
            id: hypervisor_state.id,
            asock_path: String::default(),
            state: VmmState::NotReady,
            vm_path: hypervisor_state.vm_path,
            config: hypervisor_state.config,
            netns: hypervisor_state.netns,
            pid: None,
            jailer_root: hypervisor_state.jailer_root,
            client: Client::unix(),
            pending_devices: vec![],
            run_dir: hypervisor_state.run_dir,
            capabilities: Capabilities::new(),
            fc_process: Mutex::new(None),
            exit_notify: Some(exit_notify),
        })
    }
}

#[cfg(test)]
mod minimal_tests {
    use super::*;
    #[tokio::test]
    async fn restore_requires_jailed_seccomp_enabled_state() {
        let mut state = HypervisorState::default();
        state.config.jailer_path = "/usr/bin/jailer".into();
        let (tx, _) = mpsc::channel(1);
        assert!(FcInner::restore(tx, state.clone()).await.is_err());

        state.jailed = true;
        state.config.security_info.disable_seccomp = true;
        let (tx, _) = mpsc::channel(1);
        assert!(FcInner::restore(tx, state.clone()).await.is_err());

        state.config.security_info.disable_seccomp = false;
        state.config.jailer_path.clear();
        let (tx, _) = mpsc::channel(1);
        assert!(FcInner::restore(tx, state.clone()).await.is_err());

        state.config.jailer_path = "/usr/bin/jailer".into();
        let (tx, _) = mpsc::channel(1);
        let restored = FcInner::restore(tx, state).await.unwrap();
        assert!(restored.save().await.unwrap().jailed);
    }

    #[tokio::test]
    async fn prepare_rejects_missing_jailer_before_side_effects() {
        let (tx, _) = mpsc::channel(1);
        let mut fc = FcInner::new(tx);
        assert!(fc.prepare_vm("unsupported", None, None).await.is_err());
        assert!(fc.vm_path.is_empty());
        assert!(fc.pid.is_none());
    }

    #[test]
    fn firecracker_requires_hybrid_vsock() {
        let (tx, _) = mpsc::channel(1);
        let fc = FcInner::new(tx);
        assert!(fc.capabilities.is_hybrid_vsock_supported());
        assert!(fc.capabilities.is_block_device_supported());
        assert!(!fc.capabilities.is_fs_sharing_supported());
    }
}

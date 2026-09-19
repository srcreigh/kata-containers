//Copyright (c) 2019-2022 Alibaba Cloud
//Copyright (c) 2023 Nubificus Ltd
//
//SPDX-License-Identifier: Apache-2.0

use crate::firecracker::{sl, FcInner};
use crate::{selinux, VmmState, HYPERVISOR_FIRECRACKER};
use anyhow::{anyhow, Context, Result};
use kata_types::config::KATA_PATH;
use tokio::fs;

pub const FC_API_SOCKET_NAME: &str = "fc.sock";
pub const FC_AGENT_SOCKET_NAME: &str = "kata.hvsock";
pub const ROOT: &str = "root";

const HYBRID_VSOCK_SCHEME: &str = "hvsock";

impl FcInner {
    pub(crate) async fn prepare_vm(
        &mut self,
        id: &str,
        _netns: Option<String>,
        selinux_label: Option<String>,
    ) -> Result<()> {
        debug!(sl(), "Preparing Firecracker");

        self.id = id.to_string();

        super::inner::validate_jailer_config(&self.config)?;
        self.jailer_root = KATA_PATH.to_string();
        self.vm_path = [self.jailer_root.as_str(), HYPERVISOR_FIRECRACKER, id].join("/");
        self.run_dir = [self.vm_path.as_str(), ROOT, "run"].join("/");
        let _ = self.remount_jailer_with_exec().await;
        self.asock_path = [self.run_dir.as_str(), "fc.sock"].join("/");
        debug!(sl(), "Socket Path: {:?}", self.asock_path);

        let _ = fs::create_dir_all(self.run_dir.as_str())
            .await
            .context(format!("failed to create directory {:?}", self.vm_path));

        self.netns = _netns.clone();

        if !self.hypervisor_config().disable_selinux {
            if let Some(label) = selinux_label.as_ref() {
                self.config.security_info.selinux_label = Some(label.to_string());
                selinux::set_exec_label(label).context("failed to set SELinux process label")?;
            }
        }

        self.prepare_vmm(self.netns.clone()).await?;
        self.state = VmmState::VmmServerReady;
        self.prepare_vmm_resources().await?;
        self.prepare_hvsock().await?;
        Ok(())
    }

    pub(crate) async fn start_vm(&mut self, _timeout: i32) -> Result<()> {
        debug!(sl(), "Starting sandbox");
        let body: String = serde_json::json!({
            "action_type": "InstanceStart"
        })
        .to_string();
        self.request_with_retry(hyper::Method::PUT, "/actions", body)
            .await?;
        self.state = VmmState::VmRunning;
        Ok(())
    }

    pub(crate) async fn stop_vm(&mut self) -> Result<()> {
        debug!(sl(), "Stopping sandbox");
        if self.state != VmmState::VmRunning {
            debug!(sl(), "VM not running!");
        } else {
            let mut fc_process = self.fc_process.lock().await;
            if let Some(fc_process) = fc_process.as_mut() {
                if fc_process.id().is_some() {
                    info!(sl!(), "FcInner::stop_vm(): kill()'ing fc");
                    return fc_process.kill().await.map_err(anyhow::Error::from);
                } else {
                    info!(
                        sl!(),
                        "FcInner::stop_vm(): fc process isn't running (likely stopped already)"
                    );
                }
            } else {
                info!(
                    sl!(),
                    "FcInner::stop_vm(): fc process isn't running (likely stopped already)"
                );
            }
        }

        Ok(())
    }

    pub(crate) async fn wait_vm(&self) -> Result<Option<i32>> {
        let mut process = self.fc_process.lock().await;
        let child = process
            .as_mut()
            .ok_or_else(|| anyhow!("the process has been reaped"))?;
        match child.try_wait()? {
            Some(status) => {
                let code = status.code().unwrap_or(0);
                process.take();
                Ok(Some(code))
            }
            None => Ok(None),
        }
    }

    pub(crate) async fn get_agent_socket(&self) -> Result<String> {
        debug!(sl(), "Get kata-agent socket");
        let vsock_path = [self.vm_path.as_str(), ROOT, FC_AGENT_SOCKET_NAME].join("/");
        Ok(format!("{HYBRID_VSOCK_SCHEME}://{vsock_path}"))
    }

    pub(crate) async fn get_vmm_master_tid(&self) -> Result<u32> {
        debug!(sl(), "Get VMM master TID");
        if let Some(pid) = self.pid {
            Ok(pid)
        } else {
            Err(anyhow!("could not get vmm master tid"))
        }
    }
    pub(crate) async fn cleanup(&self) -> Result<()> {
        debug!(sl(), "Cleanup");
        self.cleanup_resource();

        Ok(())
    }
}

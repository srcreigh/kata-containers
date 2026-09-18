// Copyright (c) 2019 Ant Financial
//
// SPDX-License-Identifier: Apache-2.0
//

use async_trait::async_trait;
use pathrs::flags::OpenFlags;
use rustjail::{pipestream::PipeStream, process::StreamType};
use tokio::io::{AsyncReadExt, AsyncWriteExt, ReadHalf};
use tokio::sync::Mutex;
use tokio::time::{timeout, Duration};

use std::convert::TryFrom;
use std::ffi::OsStr;
use std::fmt::Debug;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::sync::Arc;
use ttrpc::{
    self,
    error::get_rpc_status,
    r#async::{Server as TtrpcServer, TtrpcContext},
};

use anyhow::{anyhow, Context, Result};
use cgroups::FreezerState;
use oci::{LinuxNamespace, Spec};
use oci_spec::runtime as oci;
use protobuf::MessageField;
use protocols::agent::{
    AgentDetails, CopyFileRequest, GuestDetailsResponse, Metrics, OOMEvent, ReadStreamResponse,
    Routes, StatsContainerResponse, VolumeStatsRequest, WaitProcessResponse, WriteStreamResponse,
};
use protocols::csi::{
    volume_usage::Unit as VolumeUsage_Unit, VolumeCondition, VolumeStatsResponse, VolumeUsage,
};
use protocols::empty::Empty;
use protocols::health::{
    health_check_response::ServingStatus as HealthCheckResponse_ServingStatus, HealthCheckResponse,
    VersionCheckResponse,
};
use protocols::types::Interface;
use protocols::{agent_ttrpc_async as agent_ttrpc, health_ttrpc_async as health_ttrpc};
use rustjail::cgroups::notifier;
use rustjail::container::{BaseContainer, Container, LinuxContainer};
use rustjail::process::Process;
use rustjail::specconv::CreateOpts;

use nix::errno::Errno;
use nix::mount::MsFlags;
use nix::sys::{stat, statfs};
use nix::unistd::{self, Pid};
use rustjail::process::ProcessOperations;
#[cfg(test)]
use std::os::fd::AsRawFd;

use crate::features::get_build_features;
use crate::metrics::get_metrics;
use crate::mount::baremount;
use crate::namespace::{NSTYPEIPC, NSTYPEPID, NSTYPEUTS};
use crate::network::setup_guest_dns;
use crate::sandbox::{Sandbox, SandboxError};
use crate::storage::{add_storages, STORAGE_HANDLERS};
use crate::version::{AGENT_VERSION, API_VERSION};
use crate::AGENT_CONFIG;

use libc::{self, c_ushort, pid_t, winsize, TIOCSWINSZ};
use std::fs;
use std::os::unix::prelude::PermissionsExt;

use nix::unistd::{Gid, Uid};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::os::unix::fs::FileExt;
use std::path::PathBuf;

use kata_types::k8s;

pub const CONTAINER_BASE: &str = "/run/kata-containers";
const KATA_GUEST_SHARE_DIR: &str = "/run/kata-containers/shared/containers/";

const ERR_CANNOT_GET_WRITER: &str = "Cannot get writer";
const ERR_NO_LINUX_FIELD: &str = "Spec does not contain linux field";
const ERR_NO_SANDBOX_PIDNS: &str = "Sandbox does not have sandbox_pidns";

/// This mask is applied to files and directories created for CopyFile requests.
/// In addition to the permissions, it allows setuid/setgid/sticky bits.
/// Note that the setuid bit does not have an effect on Linux, though.
const FILE_PERMISSION_MASK: u32 = 0o7777;

// Validate before mutating sandbox state or touching guest files/namespaces.
fn validate_sandbox_features(req: &protocols::agent::CreateSandboxRequest) -> ttrpc::Result<()> {
    validate_storage_features(&req.storages)?;
    if !req.kernel_modules.is_empty() || !req.guest_hook_path.is_empty() {
        return Err(ttrpc_error(
            ttrpc::Code::INVALID_ARGUMENT,
            "kata-fc: guest kernel modules and guest hooks are unsupported",
        ));
    }
    Ok(())
}

// Proto3 scalar port zero means absent; reject any requested pass-fd port.
fn validate_passfd(stdin: u32, stdout: u32, stderr: u32) -> ttrpc::Result<()> {
    if stdin != 0 || stdout != 0 || stderr != 0 {
        return Err(ttrpc_error(
            ttrpc::Code::INVALID_ARGUMENT,
            "kata-fc: pass-fd IO is unsupported",
        ));
    }
    Ok(())
}

fn validate_storage_features(storages: &[protocols::agent::Storage]) -> ttrpc::Result<()> {
    crate::storage::validate_storages(storages)
        .map_err(|e| ttrpc_error(ttrpc::Code::INVALID_ARGUMENT, e.to_string()))
}

// Convenience function to obtain the scope logger.
fn sl() -> slog::Logger {
    slog_scope::logger()
}

// Convenience function to wrap an error and response to ttrpc client
pub fn ttrpc_error(code: ttrpc::Code, err: impl Debug) -> ttrpc::Error {
    get_rpc_status(code, format!("{err:?}"))
}

/// Convert SandboxError to ttrpc error with appropriate code.
/// Process not found errors map to NOT_FOUND, others to INVALID_ARGUMENT.
fn sandbox_err_to_ttrpc(err: SandboxError) -> ttrpc::Error {
    let code = match &err {
        SandboxError::InitProcessNotFound | SandboxError::InvalidExecId => ttrpc::Code::NOT_FOUND,
        SandboxError::InvalidContainerId => ttrpc::Code::INVALID_ARGUMENT,
    };
    ttrpc_error(code, err)
}

fn same<E>(e: E) -> E {
    e
}

trait ResultToTtrpcResult<T, E: Debug>: Sized {
    fn map_ttrpc_err<R: Debug>(self, msg_builder: impl FnOnce(E) -> R) -> ttrpc::Result<T>;
    fn map_ttrpc_err_do(self, doer: impl FnOnce(&E)) -> ttrpc::Result<T> {
        self.map_ttrpc_err(|e| {
            doer(&e);
            e
        })
    }
}

impl<T> ResultToTtrpcResult<T, anyhow::Error> for anyhow::Result<T> {
    fn map_ttrpc_err<R: Debug>(
        self,
        msg_builder: impl FnOnce(anyhow::Error) -> R,
    ) -> ttrpc::Result<T> {
        self.map_err(|e| match e.downcast::<ttrpc::error::Error>() {
            Ok(ttrpc_err) => ttrpc_err,
            Err(e) => ttrpc_error(ttrpc::Code::INTERNAL, msg_builder(e)),
        })
    }
}

macro_rules! impl_ttrpc_result_simple {
    ($($err_ty:ty),* $(,)?) => {
        $(
            impl<T> ResultToTtrpcResult<T, $err_ty> for Result<T, $err_ty> {
                fn map_ttrpc_err<R: Debug>(self, msg_builder: impl FnOnce($err_ty) -> R) -> ttrpc::Result<T> {
                    self.map_err(|e| ttrpc_error(ttrpc::Code::INTERNAL, msg_builder(e)))
                }
            }
        )*
    };
}

impl_ttrpc_result_simple!(
    nix::errno::Errno,
    tokio::time::error::Elapsed,
    tokio::task::JoinError,
    i32,
    std::io::Error,
);

trait OptionToTtrpcResult<T>: Sized {
    fn map_ttrpc_err(self, code: ttrpc::Code, msg: &str) -> ttrpc::Result<T>;
}

impl<T> OptionToTtrpcResult<T> for Option<T> {
    fn map_ttrpc_err(self, code: ttrpc::Code, msg: &str) -> ttrpc::Result<T> {
        self.ok_or_else(|| ttrpc_error(code, msg))
    }
}

fn validate_container_device_features(
    req: &protocols::agent::CreateContainerRequest,
) -> ttrpc::Result<()> {
    if !req.devices.is_empty() || !req.shared_mounts.is_empty() {
        return Err(ttrpc_error(
            ttrpc::Code::INVALID_ARGUMENT,
            "kata-fc: raw device passthrough and cross-container shared mounts are unsupported",
        ));
    }
    if let Some(spec) = req.OCI.as_ref() {
        let spec: Spec = spec.clone().into();
        kata_types::device::validate_spec_device_features(&spec)
            .map_err(|e| ttrpc_error(ttrpc::Code::INVALID_ARGUMENT, e.to_string()))?;
    }
    Ok(())
}

#[derive(Clone, Debug)]
pub struct AgentService {
    sandbox: Arc<Mutex<Sandbox>>,
}

impl AgentService {
    async fn do_create_container(
        &self,
        req: protocols::agent::CreateContainerRequest,
    ) -> Result<()> {
        validate_passfd(req.stdin_port, req.stdout_port, req.stderr_port)?;
        validate_storage_features(&req.storages)?;
        validate_container_device_features(&req)?;
        let cid = req.container_id.clone();

        kata_sys_util::validate::verify_id(&cid)?;

        let use_sandbox_pidns = req.sandbox_pidns();

        let mut oci = match req.OCI.into_option() {
            Some(spec) => spec.into(),
            None => {
                error!(sl(), "no oci spec in the create container request!");
                return Err(anyhow!(nix::Error::EINVAL));
            }
        };

        let container_name = k8s::container_name(&oci);

        info!(sl(), "receive createcontainer, spec: {:?}", &oci);
        info!(
            sl(),
            "receive createcontainer, storages: {:?}", &req.storages
        );

        // Both rootfs and volumes (invoked with --volume for instance) will
        // be processed the same way. The idea is to always mount any provided
        // storage to the specified MountPoint, so that it will match what's
        // inside oci.Mounts.
        // After all those storages have been processed, no matter the order
        // here, the agent will rely on rustjail (using the oci.Mounts
        // list) to bind mount all of them inside the container.
        let m = add_storages(
            sl(),
            req.storages.clone(),
            &self.sandbox,
            Some(req.container_id),
        )
        .await?;

        let mut s = self.sandbox.lock().await;
        s.container_mounts.insert(cid.clone(), m);

        update_container_namespaces(&s, &mut oci, use_sandbox_pidns)?;

        // Write the OCI spec to the container bundle.
        let olddir = setup_bundle(&cid, &mut oci)?;
        // restore the cwd for kata-agent process.
        defer!(unistd::chdir(&olddir).unwrap());

        let opts = CreateOpts {
            cgroup_name: "".to_string(),
            // The supported guest runs the agent as PID 1 without systemd.
            use_systemd_cgroup: false,
            no_pivot_root: s.no_pivot_root,
            no_new_keyring: false,
            spec: Some(oci.clone()),
            rootless_euid: false,
            rootless_cgroup: false,
            container_name,
        };

        let mut ctr: LinuxContainer = LinuxContainer::new(
            cid.as_str(),
            CONTAINER_BASE,
            Some(s.devcg_info.clone()),
            opts,
            &sl(),
        )?;

        let pipe_size = AGENT_CONFIG.container_pipe_size;

        let Some(p) = oci.process() else {
            info!(sl(), "no process configurations!");
            return Err(anyhow!(nix::Error::EINVAL));
        };

        let p = Process::new(&sl(), p, cid.as_str(), true, pipe_size)?;

        // if starting container failed, we will do some rollback work
        // to ensure no resources are leaked.
        if let Err(err) = ctr.start(p).await {
            error!(sl(), "failed to start container: {:?}", err);
            if let Err(e) = ctr.destroy().await {
                error!(sl(), "failed to destroy container: {:?}", e);
            }
            if let Err(e) = remove_container_resources(&mut s, &cid).await {
                error!(sl(), "failed to remove container resources: {:?}", e);
            }
            return Err(err);
        }

        s.update_shared_pidns(&ctr)?;
        s.add_container(ctr);
        info!(sl(), "created container!");

        Ok(())
    }

    async fn do_start_container(&self, req: protocols::agent::StartContainerRequest) -> Result<()> {
        let mut s = self.sandbox.lock().await;
        let sid = s.id.clone();
        let cid = req.container_id.clone();

        let ctr = s
            .get_container(&cid)
            .ok_or_else(|| anyhow!("Invalid container id"))?;

        if sid != cid {
            // start oom event loop
            if let Ok(cg_path) = ctr.cgroup_manager.as_ref().get_cgroup_path("memory") {
                let rx = notifier::notify_oom(cid.as_str(), cg_path.to_string()).await?;
                s.run_oom_event_monitor(rx, cid.clone()).await;
            }
        }

        let ctr = s
            .get_container(&cid)
            .ok_or_else(|| anyhow!("Invalid container id"))?;

        ctr.exec().await
    }

    async fn do_remove_container(
        &self,
        req: protocols::agent::RemoveContainerRequest,
    ) -> Result<()> {
        let cid = req.container_id;

        if req.timeout == 0 {
            let mut sandbox = self.sandbox.lock().await;
            sandbox
                .get_container(&cid)
                .ok_or_else(|| anyhow!("Invalid container id"))?
                .destroy()
                .await?;
            remove_container_resources(&mut sandbox, &cid).await?;
            return Ok(());
        }

        // timeout != 0
        let s = self.sandbox.clone();
        let cid2 = cid.clone();
        let handle = tokio::spawn(async move {
            let mut sandbox = s.lock().await;
            sandbox
                .get_container(&cid2)
                .ok_or_else(|| anyhow!("Invalid container id"))?
                .destroy()
                .await
        });

        let to = Duration::from_secs(req.timeout.into());
        tokio::time::timeout(to, handle)
            .await
            .map_err(|_| anyhow!(nix::Error::ETIME))???;

        remove_container_resources(&mut *self.sandbox.lock().await, &cid).await
    }

    async fn do_exec_process(&self, req: protocols::agent::ExecProcessRequest) -> Result<()> {
        validate_passfd(req.stdin_port, req.stdout_port, req.stderr_port)?;
        let cid = req.container_id;
        let exec_id = req.exec_id;

        info!(sl(), "do_exec_process cid: {} eid: {}", cid, exec_id);

        let mut sandbox = self.sandbox.lock().await;
        let process = req
            .process
            .into_option()
            .ok_or_else(|| anyhow!("Unable to parse process from ExecProcessRequest"))?;

        let pipe_size = AGENT_CONFIG.container_pipe_size;
        let ocip = process.into();
        let p = Process::new(&sl(), &ocip, exec_id.as_str(), false, pipe_size)?;

        let ctr = sandbox
            .get_container(&cid)
            .ok_or_else(|| anyhow!("Invalid container id"))?;

        ctr.run(p).await
    }

    async fn do_signal_process(&self, req: protocols::agent::SignalProcessRequest) -> Result<()> {
        let cid = req.container_id;
        let eid = req.exec_id;
        let mut sig: libc::c_int = req.signal as libc::c_int;

        let all = eid.is_empty() && sig == libc::SIGKILL;
        // NOTE: kata runtime encodes all = true by setting eid = "".
        // However, containerd can send eid = "", sig = SIGTERM, all = false when deleting containers
        // (and containerd will send eid = "", sig = SIGKILL, all = true when forcefully deleting containers)
        // Luckily, containerd never sends eid = "", sig = SIGKILL, all = false outside of ctr
        // So we can recover the original value of all here by checking eid and sig

        info!(
            sl(),
            "signal process";
            "container-id" => &cid,
            "exec-id" => &eid,
            "signal" => req.signal,
            "all" => all,
        );

        if !all {
            let mut sandbox = self.sandbox.lock().await;
            let p = sandbox
                .find_container_process(cid.as_str(), eid.as_str())
                .map_err(sandbox_err_to_ttrpc)?;
            // For container initProcess, if it hasn't installed handler for "SIGTERM" signal,
            // it will ignore the "SIGTERM" signal sent to it, thus send it "SIGKILL" signal
            // instead of "SIGTERM" to terminate it.
            let proc_status_file = format!("/proc/{}/status", p.pid);
            if p.init && sig == libc::SIGTERM && !is_signal_handled(&proc_status_file, sig as u32) {
                sig = libc::SIGKILL;
            }

            debug!(
                sl(),
                "signaling a container process";
                "container-id" => &cid,
                "exec-id" => &eid,
                "pid" => p.pid,
                "signal" => sig,
            );

            match p.signal(sig) {
                Err(Errno::ESRCH) => {
                    info!(
                        sl(),
                        "signal encounter ESRCH, continue";
                        "container-id" => &cid,
                        "exec-id" => &eid,
                        "pid" => p.pid,
                        "signal" => sig,
                    );
                }
                Err(err) => return Err(anyhow!(err)),
                Ok(()) => (),
            }
        } else {
            // Signalling all processes in the cgroup
            info!(
                sl(),
                "signal all the remaining processes";
                "container-id" => &cid,
                "exec-id" => &eid,
            );

            if let Err(err) = self.freeze_cgroup(&cid, FreezerState::Frozen).await {
                warn!(
                    sl(),
                    "freeze cgroup failed";
                    "container-id" => &cid,
                    "exec-id" => &eid,
                    "error" => format!("{:?}", err),
                );
            }

            let pids = self.get_pids(&cid).await?;
            for pid in pids.iter() {
                let res = unsafe { libc::kill(*pid, sig) };
                if let Err(err) = Errno::result(res).map(drop) {
                    warn!(
                        sl(),
                        "signal failed";
                        "container-id" => &cid,
                        "exec-id" => &eid,
                        "pid" => pid,
                        "error" => format!("{:?}", err),
                    );
                }
            }
            if let Err(err) = self.freeze_cgroup(&cid, FreezerState::Thawed).await {
                warn!(
                    sl(),
                    "unfreeze cgroup failed";
                    "container-id" => &cid,
                    "exec-id" => &eid,
                    "error" => format!("{:?}", err),
                );
            }
        }

        Ok(())
    }

    async fn freeze_cgroup(&self, cid: &str, state: FreezerState) -> Result<()> {
        let mut sandbox = self.sandbox.lock().await;
        let ctr = sandbox
            .get_container(cid)
            .ok_or_else(|| anyhow!("Invalid container id {}", cid))?;
        ctr.cgroup_manager.as_ref().freeze(state)
    }

    async fn get_pids(&self, cid: &str) -> Result<Vec<i32>> {
        let mut sandbox = self.sandbox.lock().await;
        let ctr = sandbox
            .get_container(cid)
            .ok_or_else(|| anyhow!("Invalid container id {}", cid))?;
        ctr.cgroup_manager.as_ref().get_pids()
    }

    async fn do_wait_process(
        &self,
        req: protocols::agent::WaitProcessRequest,
    ) -> Result<protocols::agent::WaitProcessResponse> {
        let cid = req.container_id;
        let mut eid = req.exec_id;
        let mut resp = WaitProcessResponse::new();

        info!(
            sl(),
            "wait process";
            "container-id" => &cid,
            "exec-id" => &eid
        );

        let pid: pid_t;
        let (exit_send, mut exit_recv) = tokio::sync::mpsc::channel(100);
        let exit_rx = {
            let mut sandbox = self.sandbox.lock().await;
            let p = sandbox
                .find_container_process(cid.as_str(), eid.as_str())
                .map_err(sandbox_err_to_ttrpc)?;

            p.exit_watchers.push(exit_send);
            pid = p.pid;

            p.exit_rx.clone()
        };

        if let Some(mut exit_rx) = exit_rx {
            info!(sl(), "cid {} eid {} waiting for exit signal", &cid, &eid);
            while exit_rx.changed().await.is_ok() {}
            info!(sl(), "cid {} eid {} received exit signal", &cid, &eid);
        }

        let mut sandbox = self.sandbox.lock().await;
        let ctr = sandbox
            .get_container(&cid)
            .ok_or_else(|| anyhow!("Invalid container id"))?;

        let p = match ctr.processes.values_mut().find(|p| p.pid == pid) {
            Some(p) => p,
            None => {
                // Lost race, pick up exit code from channel
                resp.status = exit_recv
                    .recv()
                    .await
                    .ok_or_else(|| anyhow!("Failed to receive exit code"))?;

                return Ok(resp);
            }
        };

        eid = p.exec_id.clone();

        // need to close all fd
        // ignore errors for some fd might be closed by stream
        p.cleanup_process_stream();

        resp.status = p.exit_code;
        // broadcast exit code to all parallel watchers
        for s in p.exit_watchers.iter_mut() {
            // Just ignore errors in case any watcher quits unexpectedly
            let _ = s.send(p.exit_code).await;
        }

        ctr.processes.remove(&eid);

        Ok(resp)
    }

    async fn do_read_termination_log(
        &self,
        container_id: &str,
    ) -> Result<protocols::agent::GetDiagnosticDataResponse> {
        let host_path = {
            let sandbox = self.sandbox.lock().await;
            let ctr = sandbox
                .containers
                .get(container_id)
                .ok_or_else(|| anyhow!("Invalid container id: {}", container_id))?;

            let spec = ctr
                .config
                .spec
                .as_ref()
                .ok_or_else(|| anyhow!("No OCI spec for container {}", container_id))?;

            let annotations = spec.annotations().as_ref();
            let termination_path = annotations
                .and_then(|a| a.get("io.kubernetes.container.terminationMessagePath"))
                .ok_or_else(|| anyhow!("No terminationMessagePath annotation"))?;

            // The path is the *container* destination (e.g. /dev/termination-log). The agent
            // runs outside the container mount namespace; the file is on the guest at the
            // bind-mount source (e.g. /run/kata-containers/shared/containers/...-termination-log).
            let term_dest = Path::new(termination_path.as_str());
            spec.mounts()
                .as_ref()
                .and_then(|mounts| {
                    mounts.iter().find_map(|m| {
                        if m.destination() == term_dest {
                            m.source().clone()
                        } else {
                            None
                        }
                    })
                })
                .ok_or_else(|| {
                    anyhow!(
                        "termination message mount not found for {}",
                        termination_path
                    )
                })?
        };

        // Kubernetes caps termination messages at 4 KiB; read raw bytes with
        // the same limit so a malicious workload cannot exhaust agent memory,
        // and handle non-UTF-8 content gracefully.
        const MAX_TERMINATION_MSG: usize = 4096;
        let contents = match tokio::fs::read(&host_path).await {
            Ok(mut buf) => {
                buf.truncate(MAX_TERMINATION_MSG);
                String::from_utf8_lossy(&buf).into_owned()
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(anyhow!("Failed to read termination log: {}", e)),
        };

        let mut resp = protocols::agent::GetDiagnosticDataResponse::new();
        resp.data = contents;
        Ok(resp)
    }

    async fn do_write_stream(
        &self,
        req: protocols::agent::WriteStreamRequest,
    ) -> Result<protocols::agent::WriteStreamResponse> {
        let cid = req.container_id;
        let eid = req.exec_id;

        let mut resp = WriteStreamResponse::new();
        resp.set_len(req.data.len() as u32);

        // EOF of stdin
        if req.data.is_empty() {
            let mut sandbox = self.sandbox.lock().await;
            let p = sandbox.find_container_process(cid.as_str(), eid.as_str())?;
            p.close_stdin().await;
        } else {
            let writer = {
                let mut sandbox = self.sandbox.lock().await;
                let p = sandbox.find_container_process(cid.as_str(), eid.as_str())?;

                // use ptmx io
                if p.term_master.is_some() {
                    p.get_writer(StreamType::TermMaster)
                } else {
                    // use piped io
                    p.get_writer(StreamType::ParentStdin)
                }
            };

            let writer = writer.ok_or_else(|| anyhow!(ERR_CANNOT_GET_WRITER))?;
            writer.lock().await.write_all(req.data.as_slice()).await?;
        }

        Ok(resp)
    }

    async fn do_read_stream(
        &self,
        req: &protocols::agent::ReadStreamRequest,
        stdout: bool,
    ) -> Result<protocols::agent::ReadStreamResponse> {
        let cid = &req.container_id;
        let eid = &req.exec_id;

        let term_exit_notifier;
        let reader = {
            let mut sandbox = self.sandbox.lock().await;
            let p = sandbox
                .find_container_process(cid.as_str(), eid.as_str())
                .map_err(sandbox_err_to_ttrpc)?;

            term_exit_notifier = p.term_exit_notifier.clone();

            if p.term_master.is_some() {
                p.get_reader(StreamType::TermMaster)
            } else if stdout {
                if p.parent_stdout.is_some() {
                    p.get_reader(StreamType::ParentStdout)
                } else {
                    None
                }
            } else {
                p.get_reader(StreamType::ParentStderr)
            }
        };

        let reader = reader.ok_or_else(|| anyhow!("cannot get stream reader"))?;

        // Create one in-flight read future and reuse it in both branches.
        let read_fut = read_stream(&reader, req.len as usize);
        tokio::pin!(read_fut);

        // Cancellation and polling model: Rust async is polled, not preempted.
        // `Future::poll()` is a synchronous function call that runs to completion and
        // returns Ready or Pending (`std::future::Future`).
        // Readiness notifications (Waker::wake / Tokio Notify) only schedule the task
        // to be polled again later; they do not interrupt an in-progress poll.
        // Therefore, a Notify becoming ready while `poll_read()` is executing cannot cause
        // the read future to be dropped mid-way; cancellation can only happen when the branch
        // is still pending between polls (Tokio `select!` cancels by dropping non-selected futures).
        // Detailed information, please refer to Tokio doc for more information:
        // - Future::poll: https://doc.rust-lang.org/std/future/trait.Future.html
        // - Waker: https://doc.rust-lang.org/std/task/struct.Waker.html
        // - Tokio select!: https://docs.rs/tokio/latest/tokio/macro.select.html
        let data = tokio::select! {
            // Use `biased` to make the polling order deterministic (top-to-bottom).
            // This ensures that *when multiple branches are ready at the same time*,
            // we prefer reading pending output over reacting to the exit notification.
            //
            // Note: `biased` does NOT guarantee that we won't lose output. If the exit
            // notification becomes ready while `read_stream` is still pending, the
            // exit branch may be selected and we may stop reading before draining the
            // remaining buffered data.
            //
            // Detailed information, please refer to Tokio doc for more information:
            // https://docs.rs/tokio/latest/src/tokio/macros/select.rs.html#67
            biased;

            v = &mut read_fut => v?,
            _ = term_exit_notifier.notified() => {
                // Drain-after-exit rationale:
                // The process has exited, but the data may still be buffered in the pipe/pty.
                // We should keep waiting for the same in-flight read for a bounded window to drain the data.
                //
                // It enters this branch only if `term_exit_notifier.notified()` fires. It then try to "drain"
                // any remaining buffered output for a short, bounded time window:
                // - If non-empty data is read: return immediately.
                // - else then return empty data as EOF.

                const DRAIN_DEADLINE_MS: u64 = 500; // 500ms
                let deadline = Duration::from_millis(DRAIN_DEADLINE_MS);

                // Attempt to drain remaining buffered output after process exit
                // Try reading with timeout
                match timeout(deadline, &mut read_fut).await {
                    Ok(v) => v?, // got data or EOF (empty)
                    _ => {
                        warn!(sl(), "exit-drain timeout, return EOF"; "container-id" => cid, "exec-id" => eid);
                        Vec::new() // Return empty as EOF
                    }
                }
            }
        };

        let mut resp = ReadStreamResponse::new();
        resp.set_data(data);

        Ok(resp)
    }
}

#[async_trait]
impl agent_ttrpc::AgentService for AgentService {
    async fn create_container(
        &self,
        _ctx: &TtrpcContext,
        req: protocols::agent::CreateContainerRequest,
    ) -> ttrpc::Result<Empty> {
        info!(sl(), "rpc call from shim to agent: {}", "create_container");
        self.do_create_container(req).await.map_ttrpc_err(same)?;
        Ok(Empty::new())
    }

    async fn start_container(
        &self,
        _ctx: &TtrpcContext,
        req: protocols::agent::StartContainerRequest,
    ) -> ttrpc::Result<Empty> {
        info!(sl(), "rpc call from shim to agent: {}", "start_container");
        self.do_start_container(req).await.map_ttrpc_err(same)?;
        Ok(Empty::new())
    }

    async fn remove_container(
        &self,
        _ctx: &TtrpcContext,
        req: protocols::agent::RemoveContainerRequest,
    ) -> ttrpc::Result<Empty> {
        info!(sl(), "rpc call from shim to agent: {}", "remove_container");
        self.do_remove_container(req).await.map_ttrpc_err(same)?;
        Ok(Empty::new())
    }

    async fn exec_process(
        &self,
        _ctx: &TtrpcContext,
        req: protocols::agent::ExecProcessRequest,
    ) -> ttrpc::Result<Empty> {
        info!(sl(), "rpc call from shim to agent: {}", "exec_process");
        self.do_exec_process(req).await.map_ttrpc_err(same)?;
        Ok(Empty::new())
    }

    async fn signal_process(
        &self,
        _ctx: &TtrpcContext,
        req: protocols::agent::SignalProcessRequest,
    ) -> ttrpc::Result<Empty> {
        info!(sl(), "rpc call from shim to agent: {}", "signal_process");
        self.do_signal_process(req).await.map_ttrpc_err(same)?;
        Ok(Empty::new())
    }

    async fn wait_process(
        &self,
        _ctx: &TtrpcContext,
        req: protocols::agent::WaitProcessRequest,
    ) -> ttrpc::Result<WaitProcessResponse> {
        info!(sl(), "rpc call from shim to agent: {}", "wait_process");
        self.do_wait_process(req).await.map_ttrpc_err(same)
    }

    async fn update_container(
        &self,
        _ctx: &TtrpcContext,
        req: protocols::agent::UpdateContainerRequest,
    ) -> ttrpc::Result<Empty> {
        info!(sl(), "rpc call from shim to agent: {}", "update_container");

        let mut sandbox = self.sandbox.lock().await;
        let ctr = sandbox
            .get_container(&req.container_id)
            .map_ttrpc_err(ttrpc::Code::INVALID_ARGUMENT, "invalid container id")?;
        if let Some(res) = req.resources.as_ref() {
            let oci_res = res.clone().into();
            ctr.set(oci_res).map_ttrpc_err(same)?;
        }

        Ok(Empty::new())
    }

    async fn stats_container(
        &self,
        _ctx: &TtrpcContext,
        req: protocols::agent::StatsContainerRequest,
    ) -> ttrpc::Result<StatsContainerResponse> {
        info!(sl(), "rpc call from shim to agent: {}", "stats_container");

        // Clone the container's cgroup manager (an Arc) while holding the sandbox lock, then
        // release the lock BEFORE the blocking cgroup read below.
        //
        // The sandbox lock is a single global mutex taken by nearly every agent RPC,
        // including the signal_process/kill path. Previously stats_container held it across
        // the synchronous get_stats() cgroup read; if that read blocked on a container whose
        // cgroup is being torn down (e.g. during pod termination), it serialized the whole
        // agent and starved the kill RPC, leaving the pod stuck Terminating with the host
        // seeing ttrpc "Receive packet timeout". Cloning the Arc lets stats read the cgroup
        // without blocking any other RPC.
        let cgroup_manager = {
            let mut sandbox = self.sandbox.lock().await;
            let ctr = sandbox
                .get_container(&req.container_id)
                .map_ttrpc_err(ttrpc::Code::INVALID_ARGUMENT, "invalid container id")?;
            ctr.cgroup_manager.clone()
        };

        // get_stats() performs blocking synchronous cgroup file reads; run it on the blocking
        // thread pool so a slow/stuck read cannot park an async reactor worker.
        let cgroup_stats = tokio::task::spawn_blocking(move || cgroup_manager.get_stats())
            .await
            .map_ttrpc_err(same)?
            .map_ttrpc_err(same)?;

        Ok(StatsContainerResponse {
            cgroup_stats: MessageField::some(cgroup_stats),
            ..Default::default()
        })
    }

    async fn pause_container(
        &self,
        _ctx: &TtrpcContext,
        req: protocols::agent::PauseContainerRequest,
    ) -> ttrpc::Result<protocols::empty::Empty> {
        info!(sl(), "rpc call from shim to agent: {}", "pause_container");

        let mut sandbox = self.sandbox.lock().await;
        let ctr = sandbox
            .get_container(&req.container_id)
            .map_ttrpc_err(ttrpc::Code::INVALID_ARGUMENT, "invalid container id")?;
        ctr.pause().map_ttrpc_err(same)?;
        Ok(Empty::new())
    }

    async fn resume_container(
        &self,
        _ctx: &TtrpcContext,
        req: protocols::agent::ResumeContainerRequest,
    ) -> ttrpc::Result<protocols::empty::Empty> {
        info!(sl(), "rpc call from shim to agent: {}", "resume_container");

        let mut sandbox = self.sandbox.lock().await;
        let ctr = sandbox
            .get_container(&req.container_id)
            .map_ttrpc_err(ttrpc::Code::INVALID_ARGUMENT, "invalid container id")?;
        ctr.resume().map_ttrpc_err(same)?;
        Ok(Empty::new())
    }

    async fn write_stdin(
        &self,
        _ctx: &TtrpcContext,
        req: protocols::agent::WriteStreamRequest,
    ) -> ttrpc::Result<WriteStreamResponse> {
        self.do_write_stream(req).await.map_ttrpc_err(same)
    }

    async fn read_stdout(
        &self,
        _ctx: &TtrpcContext,
        req: protocols::agent::ReadStreamRequest,
    ) -> ttrpc::Result<ReadStreamResponse> {
        self.do_read_stream(&req, true).await.map_ttrpc_err(same)
    }

    async fn read_stderr(
        &self,
        _ctx: &TtrpcContext,
        req: protocols::agent::ReadStreamRequest,
    ) -> ttrpc::Result<ReadStreamResponse> {
        self.do_read_stream(&req, false).await.map_ttrpc_err(same)
    }

    async fn tty_win_resize(
        &self,
        _ctx: &TtrpcContext,
        req: protocols::agent::TtyWinResizeRequest,
    ) -> ttrpc::Result<Empty> {
        info!(sl(), "rpc call from shim to agent: {}", "tty_win_resize");

        let mut sandbox = self.sandbox.lock().await;
        let p = sandbox
            .find_container_process(req.container_id(), req.exec_id())
            .map_err(sandbox_err_to_ttrpc)?;

        let fd = p
            .term_master
            .map_ttrpc_err(ttrpc::Code::UNAVAILABLE, "no tty")?;
        let win = winsize {
            ws_row: req.row as c_ushort,
            ws_col: req.column as c_ushort,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };

        let err = unsafe { libc::ioctl(fd, TIOCSWINSZ, &win) };
        Errno::result(err)
            .map(drop)
            .map_ttrpc_err(|e| format!("ioctl error: {e:?}"))?;

        Ok(Empty::new())
    }

    async fn update_interface(
        &self,
        _ctx: &TtrpcContext,
        req: protocols::agent::UpdateInterfaceRequest,
    ) -> ttrpc::Result<Interface> {
        info!(sl(), "rpc call from shim to agent: {}", "update_interface");

        let interface = req.interface.into_option().map_ttrpc_err(
            ttrpc::Code::INVALID_ARGUMENT,
            "empty update interface request",
        )?;

        if !interface.devicePath.is_empty() {
            return Err(ttrpc_error(
                ttrpc::Code::INVALID_ARGUMENT,
                "kata-fc: PCI/CCW network device paths are unsupported",
            ));
        }
        let mut sandbox = self.sandbox.lock().await;

        sandbox
            .rtnl
            .update_interface(&interface)
            .await
            .map_ttrpc_err(|e| format!("update interface: {e:?}"))?;

        Ok(interface)
    }

    async fn update_routes(
        &self,
        _ctx: &TtrpcContext,
        req: protocols::agent::UpdateRoutesRequest,
    ) -> ttrpc::Result<Routes> {
        info!(sl(), "rpc call from shim to agent: {}", "update_routes");

        let new_routes = req
            .routes
            .into_option()
            .map(|r| r.Routes)
            .map_ttrpc_err(ttrpc::Code::INVALID_ARGUMENT, "empty update routes request")?;

        let mut sandbox = self.sandbox.lock().await;

        sandbox
            .rtnl
            .update_routes(new_routes)
            .await
            .map_ttrpc_err(|e| format!("Failed to update routes: {e:?}"))?;

        let list = sandbox
            .rtnl
            .list_routes()
            .await
            .map_ttrpc_err(|e| format!("Failed to list routes after update: {e:?}"))?;

        Ok(protocols::agent::Routes {
            Routes: list,
            ..Default::default()
        })
    }

    async fn create_sandbox(
        &self,
        _ctx: &TtrpcContext,
        req: protocols::agent::CreateSandboxRequest,
    ) -> ttrpc::Result<Empty> {
        info!(sl(), "rpc call from shim to agent: {}", "create_sandbox");
        validate_sandbox_features(&req)?;

        {
            let mut s = self.sandbox.lock().await;

            let _ = fs::remove_dir_all(CONTAINER_BASE);
            fs::create_dir_all(KATA_GUEST_SHARE_DIR).map_ttrpc_err(same)?;

            s.hostname = req.hostname.clone();
            s.running = true;

            if !req.sandbox_id.is_empty() {
                s.id = req.sandbox_id.clone();
            }

            s.setup_shared_namespaces().await.map_ttrpc_err(same)?;
        }

        let m = add_storages(sl(), req.storages.clone(), &self.sandbox, None)
            .await
            .map_ttrpc_err(same)?;
        self.sandbox.lock().await.mounts = m;

        setup_guest_dns(sl(), &req.dns).map_ttrpc_err(same)?;
        {
            let mut s = self.sandbox.lock().await;
            for dns in req.dns {
                s.network.set_dns(dns);
            }
        }

        Ok(Empty::new())
    }

    async fn destroy_sandbox(
        &self,
        _ctx: &TtrpcContext,
        _req: protocols::agent::DestroySandboxRequest,
    ) -> ttrpc::Result<Empty> {
        info!(sl(), "rpc call from shim to agent: {}", "destroy_sandbox");

        let mut sandbox = self.sandbox.lock().await;
        // destroy all containers, clean up, notify agent to exit etc.
        sandbox.destroy().await.map_ttrpc_err(same)?;
        // Close get_oom_event connection,
        // otherwise it will block the shutdown of ttrpc.
        drop(sandbox.event_tx.take());

        sandbox
            .sender
            .take()
            .map_ttrpc_err(
                ttrpc::Code::INTERNAL,
                "failed to get sandbox sender channel",
            )?
            .send(1)
            .map_ttrpc_err(same)?;

        Ok(Empty::new())
    }

    async fn add_arp_neighbors(
        &self,
        _ctx: &TtrpcContext,
        req: protocols::agent::AddARPNeighborsRequest,
    ) -> ttrpc::Result<Empty> {
        info!(sl(), "rpc call from shim to agent: {}", "add_arp_neighbors");

        let neighs = req
            .neighbors
            .into_option()
            .map(|n| n.ARPNeighbors)
            .map_ttrpc_err(
                ttrpc::Code::INVALID_ARGUMENT,
                "empty add arp neighbours request",
            )?;

        self.sandbox
            .lock()
            .await
            .rtnl
            .add_arp_neighbors(neighs)
            .await
            .map_ttrpc_err(|e| format!("Failed to add ARP neighbours: {e:?}"))?;

        Ok(Empty::new())
    }

    async fn get_guest_details(
        &self,
        _ctx: &TtrpcContext,
        req: protocols::agent::GuestDetailsRequest,
    ) -> ttrpc::Result<GuestDetailsResponse> {
        info!(sl(), "rpc call from shim to agent: {}", "get_guest_details");

        info!(sl(), "get guest details!");
        let mut resp = GuestDetailsResponse::new();
        if req.mem_block_size || req.mem_hotplug_probe {
            return Err(ttrpc_error(
                ttrpc::Code::INVALID_ARGUMENT,
                "kata-fc: guest memory hotplug information is unsupported",
            ));
        }

        // to get agent details
        let detail = get_agent_details();
        resp.agent_details = MessageField::some(detail);

        Ok(resp)
    }

    async fn copy_file(
        &self,
        _ctx: &TtrpcContext,
        req: protocols::agent::CopyFileRequest,
    ) -> ttrpc::Result<Empty> {
        info!(sl(), "rpc call from shim to agent: {}", "copy_file");
        // Potentially untrustworthy data from the host needs to go into the shared dir.
        let root_path = PathBuf::from(KATA_GUEST_SHARE_DIR);
        do_copy_file(&req, &root_path).map_ttrpc_err(same)?;

        Ok(Empty::new())
    }

    async fn get_metrics(
        &self,
        _ctx: &TtrpcContext,
        req: protocols::agent::GetMetricsRequest,
    ) -> ttrpc::Result<Metrics> {
        info!(sl(), "rpc call from shim to agent: {}", "get_metrics");

        let s = get_metrics(&req).map_ttrpc_err(same)?;
        let mut metrics = Metrics::new();
        metrics.set_metrics(s);
        Ok(metrics)
    }

    async fn get_oom_event(
        &self,
        _ctx: &TtrpcContext,
        _req: protocols::agent::GetOOMEventRequest,
    ) -> ttrpc::Result<OOMEvent> {
        let event_rx = {
            let s = self.sandbox.lock().await;
            s.event_rx.clone()
        };
        let mut event_rx = event_rx.lock().await;

        let container_id = event_rx
            .recv()
            .await
            .map_ttrpc_err(ttrpc::Code::INTERNAL, "")?;

        info!(sl(), "get_oom_event return {}", &container_id);

        let mut resp = OOMEvent::new();
        resp.container_id = container_id;
        Ok(resp)
    }

    async fn get_volume_stats(
        &self,
        _ctx: &TtrpcContext,
        req: VolumeStatsRequest,
    ) -> ttrpc::Result<VolumeStatsResponse> {
        info!(sl(), "rpc call from shim to agent: {}", "get_volume_stats");

        info!(sl(), "get volume stats!");
        let mut resp = VolumeStatsResponse::new();
        let mut condition = VolumeCondition::new();

        File::open(&req.volume_guest_path)
            .map_ttrpc_err_do(|_| info!(sl(), "failed to open the volume"))?;

        condition.abnormal = false;
        condition.message = String::from("OK");

        let mut usage_vec = Vec::new();

        // to get volume capacity stats
        get_volume_capacity_stats(&req.volume_guest_path)
            .map(|u| usage_vec.push(u))
            .map_ttrpc_err(same)?;

        // to get volume inode stats
        get_volume_inode_stats(&req.volume_guest_path)
            .map(|u| usage_vec.push(u))
            .map_ttrpc_err(same)?;

        resp.usage = usage_vec;
        resp.volume_condition = MessageField::some(condition);
        Ok(resp)
    }

    async fn get_diagnostic_data(
        &self,
        _ctx: &TtrpcContext,
        req: protocols::agent::GetDiagnosticDataRequest,
    ) -> ttrpc::Result<protocols::agent::GetDiagnosticDataResponse> {
        info!(
            sl(),
            "rpc call from shim to agent: {}", "get_diagnostic_data"
        );

        match req.log_type.as_str() {
            "termination_log" => self
                .do_read_termination_log(&req.container_id)
                .await
                .map_ttrpc_err(same),
            other => Err(ttrpc_error(
                ttrpc::Code::INVALID_ARGUMENT,
                format!("unsupported diagnostic log_type: {other}"),
            )),
        }
    }
}

#[derive(Clone)]
struct HealthService;

#[async_trait]
impl health_ttrpc::Health for HealthService {
    async fn check(
        &self,
        _ctx: &TtrpcContext,
        _req: protocols::health::CheckRequest,
    ) -> ttrpc::Result<HealthCheckResponse> {
        let mut resp = HealthCheckResponse::new();
        resp.set_status(HealthCheckResponse_ServingStatus::SERVING);

        Ok(resp)
    }

    async fn version(
        &self,
        _ctx: &TtrpcContext,
        req: protocols::health::CheckRequest,
    ) -> ttrpc::Result<VersionCheckResponse> {
        info!(sl(), "version {:?}", req);
        let mut rep = protocols::health::VersionCheckResponse::new();
        rep.agent_version = AGENT_VERSION.to_string();
        rep.grpc_version = API_VERSION.to_string();

        Ok(rep)
    }
}

fn get_volume_capacity_stats(path: &str) -> Result<VolumeUsage> {
    let mut usage = VolumeUsage::new();

    let stat = statfs::statfs(path)?;
    let block_size = stat.block_size() as u64;
    usage.total = stat.blocks() * block_size;
    usage.available = stat.blocks_free() * block_size;
    usage.used = usage.total - usage.available;
    usage.unit = VolumeUsage_Unit::BYTES.into();

    Ok(usage)
}

fn get_volume_inode_stats(path: &str) -> Result<VolumeUsage> {
    let mut usage = VolumeUsage::new();

    let stat = statfs::statfs(path)?;
    usage.total = stat.files();
    usage.available = stat.files_free();
    usage.used = usage.total - usage.available;
    usage.unit = VolumeUsage_Unit::INODES.into();

    Ok(usage)
}

pub fn have_seccomp() -> bool {
    if cfg!(feature = "seccomp") {
        return true;
    }

    false
}

fn get_agent_details() -> AgentDetails {
    let mut detail = AgentDetails::new();

    detail.set_version(AGENT_VERSION.to_string());
    detail.set_supports_seccomp(have_seccomp());
    detail.init_daemon = unistd::getpid() == Pid::from_raw(1);

    detail.device_handlers = Vec::new();
    detail.storage_handlers = STORAGE_HANDLERS.get_handlers();
    detail.extra_features = get_build_features();

    detail
}

async fn read_stream(reader: &Mutex<ReadHalf<PipeStream>>, l: usize) -> Result<Vec<u8>> {
    let mut content = vec![0u8; l];

    let mut reader = reader.lock().await;
    let len = reader.read(&mut content).await?;
    content.resize(len, 0);

    Ok(content)
}

pub async fn start(s: Arc<Mutex<Sandbox>>, server_address: &str) -> Result<TtrpcServer> {
    let agent_service = Box::new(AgentService { sandbox: s });
    let aservice = agent_ttrpc::create_agent_service(Arc::new(*agent_service));

    let health_service = Box::new(HealthService {});
    let hservice = health_ttrpc::create_health(Arc::new(*health_service));

    let server = TtrpcServer::new()
        .bind(server_address)?
        .register_service(aservice)
        .register_service(hservice);

    info!(sl(), "ttRPC server started"; "address" => server_address);

    Ok(server)
}

// This function updates the container namespaces configuration based on the
// sandbox information. When the sandbox is created, it can be setup in a way
// that all containers will share some specific namespaces. This is the agent
// responsibility to create those namespaces so that they can be shared across
// several containers.
// If the sandbox has not been setup to share namespaces, then we assume all
// containers will be started in their own new namespace.
// The value of a.sandbox.sharedPidNs.path will always override the namespace
// path set by the spec, since we will always ignore it. Indeed, it makes no
// sense to rely on the namespace path provided by the host since namespaces
// are different inside the guest.
fn update_container_namespaces(
    sandbox: &Sandbox,
    spec: &mut Spec,
    sandbox_pidns: bool,
) -> Result<()> {
    let linux = spec
        .linux_mut()
        .as_mut()
        .ok_or_else(|| anyhow!(ERR_NO_LINUX_FIELD))?;

    if let Some(namespaces) = linux.namespaces_mut() {
        for namespace in namespaces.iter_mut() {
            if namespace.typ().to_string() == NSTYPEIPC {
                namespace.set_path(if !sandbox.shared_ipcns.path.is_empty() {
                    Some(PathBuf::from(&sandbox.shared_ipcns.path))
                } else {
                    None
                });
                continue;
            }
            if namespace.typ().to_string() == NSTYPEUTS {
                namespace.set_path(if !sandbox.shared_utsns.path.is_empty() {
                    Some(PathBuf::from(&sandbox.shared_utsns.path))
                } else {
                    None
                });
                continue;
            }
        }

        // update pid namespace
        let mut pid_ns = LinuxNamespace::default();
        pid_ns.set_typ(oci::LinuxNamespaceType::try_from(NSTYPEPID).unwrap());

        // Use shared pid ns if useSandboxPidns has been set in either
        // the create_sandbox request or create_container request.
        // Else set this to empty string so that a new pid namespace is
        // created for the container.
        if sandbox_pidns {
            if let Some(ref pidns) = &sandbox.sandbox_pidns {
                if !pidns.path.is_empty() {
                    pid_ns.set_path(Some(PathBuf::from(&pidns.path)));
                }
            } else if !sandbox.containers.is_empty() {
                return Err(anyhow!(ERR_NO_SANDBOX_PIDNS));
            }
        }

        namespaces.push(pid_ns);
    }

    Ok(())
}

async fn remove_container_resources(sandbox: &mut Sandbox, cid: &str) -> Result<()> {
    let mut cmounts: Vec<String> = vec![];

    // Find the sandbox storage used by this container
    let mounts = sandbox.container_mounts.get(cid);
    if let Some(mounts) = mounts {
        for m in mounts.iter() {
            if sandbox.storages.contains_key(m) {
                cmounts.push(m.to_string());
            }
        }
    }

    for m in cmounts.iter() {
        if let Err(err) = sandbox.remove_sandbox_storage(m).await {
            error!(
                sl(),
                "failed to unset_and_remove_sandbox_storage for container {}, error: {:?}",
                cid,
                err
            );
        }
    }

    sandbox.container_mounts.remove(cid);
    sandbox.containers.remove(cid);
    Ok(())
}

// Check if the container process installed the
// handler for specific signal.
fn is_signal_handled(proc_status_file: &str, signum: u32) -> bool {
    let shift_count: u64 = if signum == 0 {
        // signum 0 is used to check for process liveness.
        // Since that signal is not part of the mask in the file, we only need
        // to know if the file (and therefore) process exists to handle
        // that signal.
        return fs::metadata(proc_status_file).is_ok();
    } else if signum > 64 {
        // Ensure invalid signum won't break bit shift logic
        warn!(sl(), "received invalid signum {}", signum);
        return false;
    } else {
        (signum - 1).into()
    };

    // Open the file in read-only mode (ignoring errors).
    let file = match File::open(proc_status_file) {
        Ok(f) => f,
        Err(_) => {
            warn!(sl(), "failed to open file {}", proc_status_file);
            return false;
        }
    };

    let sig_mask: u64 = 1 << shift_count;
    let reader = BufReader::new(file);

    // read lines start with SigBlk/SigIgn/SigCgt and check any match the signal mask
    reader
        .lines()
        .map_while(Result::ok)
        .filter(|line| {
            line.starts_with("SigBlk:")
                || line.starts_with("SigIgn:")
                || line.starts_with("SigCgt:")
        })
        .any(|line| {
            let mask_vec: Vec<&str> = line.split(':').collect();
            if mask_vec.len() == 2 {
                let sig_str = mask_vec[1].trim();
                if let Ok(sig) = u64::from_str_radix(sig_str, 16) {
                    return sig & sig_mask == sig_mask;
                }
            }
            false
        })
}

/// do_copy_file creates a file, directory or symlink beneath the provided directory.
///
/// The function guarantees that no content is written outside of the directory. However, a symlink
/// created by this function might point outside the shared directory. Other users of that
/// directory need to consider whether they trust the host, or handle the directory with the same
/// care as do_copy_file.
///
/// The shared root and all parent directories must already exist. Only the requested target
/// receives req.file_mode and ownership; req.dir_mode is unused. Directories must be explicitly
/// created before copying their children.
///
/// If req.file_mode requests a symbolic link, the link is created pointing to the path in
/// req.data. In that case, req.file_mode is ignored because symlinks don't have permissions on
/// Linux.
///
/// If this function returns an error, the filesystem may be in an unexpected state. This is not
/// significant for the caller, since errors are almost certainly not retriable. The runtime should
/// abandon this VM instead.
fn do_copy_file(req: &CopyFileRequest, shared_dir: &PathBuf) -> Result<()> {
    let insecure_full_path = PathBuf::from(req.path.as_str());
    let path = insecure_full_path
        .strip_prefix(shared_dir)
        .context(format!(
            "removing {:?} prefix from {}",
            shared_dir, req.path
        ))?;

    let root = pathrs::Root::open(shared_dir)?;

    let sflag = stat::SFlag::from_bits_truncate(req.file_mode);

    if sflag.contains(stat::SFlag::S_IFDIR) {
        // Directories are somewhat special: for backwards compatibility, we need to preserve an
        // existing directory at path, so we can't just remove_all. Instead, we try to remove a
        // file and just don't propagate the error if it's a directory or doesn't exist.
        root.remove_file(path).or_else(|e| match e.kind() {
            pathrs::error::ErrorKind::OsError(Some(errno))
                if errno == libc::ENOENT || errno == libc::EISDIR =>
            {
                Ok(())
            }
            _ => Err(e),
        })?;

        // Create only the requested directory; never synthesize its parents.
        root.create(
            path,
            &pathrs::InodeType::Directory(std::fs::Permissions::from_mode(
                req.file_mode & FILE_PERMISSION_MASK,
            )),
        )
        .or_else(|e| match e.kind() {
            pathrs::error::ErrorKind::OsError(Some(libc::EEXIST)) => Ok(()),
            _ => Err(e),
        })
        .context("create dir")?;
        let dir = root
            .resolve(path)?
            .reopen(OpenFlags::O_DIRECTORY)
            .context("reopen dir")?;
        dir.set_permissions(std::fs::Permissions::from_mode(
            req.file_mode & FILE_PERMISSION_MASK,
        ))?;

        unistd::fchown(
            dir,
            Some(Uid::from_raw(req.uid as u32)),
            Some(Gid::from_raw(req.gid as u32)),
        )
        .context("fchown dir")?;

        return Ok(());
    }

    // Remove any existing file if we're not resuming a chunked upload.
    if req.offset == 0 {
        // Remove anything that might already exist at the target location.
        // This is safe even for a symlink leaf, remove_all removes the named inode in its parent dir.
        root.remove_all(path).or_else(|e| match e.kind() {
            pathrs::error::ErrorKind::OsError(Some(errno)) if errno == libc::ENOENT => Ok(()),
            _ => Err(e),
        })?;
    }

    // Handle symlink creation
    if sflag.contains(stat::SFlag::S_IFLNK) {
        // Create new symbolic link
        let symlink_target = PathBuf::from(OsStr::from_bytes(&req.data));
        root.create(path, &pathrs::InodeType::Symlink(symlink_target))
            .context("create symlink")?;

        // Set symlink ownership.
        // At the time of writing this, there was no API for creating the symlink and opening a
        // handle to the created inode. Best we can do is to resolve it again under the root and
        // hope that its still the same inode, but at least we guarantee that we're changing
        // ownership only within the shared directory.
        nix::unistd::fchownat(
            root,
            path,
            Some(Uid::from_raw(req.uid as u32)),
            Some(Gid::from_raw(req.gid as u32)),
            nix::fcntl::AtFlags::AT_SYMLINK_NOFOLLOW,
        )
        .context("fchownat")?;

        // Symlinks don't have permissions on Linux!
        return Ok(());
    }

    let mut tmpfile = path.to_path_buf();
    tmpfile.set_extension("tmp");

    // Write file content.
    let flags = if req.offset == 0 {
        OpenFlags::O_RDWR | OpenFlags::O_CREAT | OpenFlags::O_TRUNC
    } else {
        OpenFlags::O_RDWR | OpenFlags::O_CREAT
    };
    let file = root
        .create_file(
            &tmpfile,
            flags,
            &std::fs::Permissions::from_mode(req.file_mode & FILE_PERMISSION_MASK),
        )
        .context("create_file")?;
    file.write_all_at(req.data.as_slice(), req.offset as u64)
        .context("write_all_at")?;

    // Check whether we're waiting for more data.

    let st = nix::sys::stat::fstat(&file).context("fstat")?;
    if st.st_size != req.file_size {
        return Ok(());
    }

    // Things like umask can change the permissions after create, make sure that they stay
    file.set_permissions(std::fs::Permissions::from_mode(
        req.file_mode & FILE_PERMISSION_MASK,
    ))
    .context("set_permissions")?;

    unistd::fchown(
        file,
        Some(Uid::from_raw(req.uid as u32)),
        Some(Gid::from_raw(req.gid as u32)),
    )
    .context("fchown")?;

    nix::fcntl::renameat(&root, &tmpfile, &root, path).context("renameat")?;

    Ok(())
}

// Setup container bundle under CONTAINER_BASE, which is cleaned up
// before removing a container.
// - bundle path is /<CONTAINER_BASE>/<cid>/
// - config.json at /<CONTAINER_BASE>/<cid>/config.json
// - container rootfs bind mounted at /<CONTAINER_BASE>/<cid>/rootfs
// - modify container spec root to point to /<CONTAINER_BASE>/<cid>/rootfs
pub fn setup_bundle(cid: &str, spec: &mut Spec) -> Result<PathBuf> {
    let spec_root = if let Some(sr) = &spec.root() {
        sr
    } else {
        return Err(anyhow!(nix::Error::EINVAL));
    };

    let bundle_path = Path::new(CONTAINER_BASE).join(cid);
    let config_path = bundle_path.join("config.json");
    let rootfs_path = bundle_path.join("rootfs");
    let spec_root_path = spec_root.path();

    let rootfs_exists = Path::new(&rootfs_path).exists();
    info!(
        sl(),
        "The rootfs_path is {:?} and exists: {}", rootfs_path, rootfs_exists
    );

    if !rootfs_exists {
        fs::create_dir_all(&rootfs_path)?;
        baremount(
            spec_root_path,
            &rootfs_path,
            "bind",
            MsFlags::MS_BIND,
            "",
            &sl(),
        )?;
    }

    let mut oci_root = oci::Root::default();
    oci_root.set_path(rootfs_path);
    oci_root.set_readonly(spec_root.readonly());
    spec.set_root(Some(oci_root));

    let _ = spec.save(
        config_path
            .to_str()
            .ok_or_else(|| anyhow!("cannot convert path to unicode"))?,
    );

    let olddir = unistd::getcwd().context("cannot getcwd")?;
    unistd::chdir(
        bundle_path
            .to_str()
            .ok_or_else(|| anyhow!("cannot convert bundle path to unicode"))?,
    )?;

    Ok(olddir)
}

#[cfg(test)]
#[allow(dead_code)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;
    use crate::{namespace::Namespace, protocols::agent_ttrpc_async::AgentService as _};
    use anyhow::{bail, ensure};
    use nix::mount;
    use oci::{
        Linux, LinuxBuilder, LinuxDeviceCgroupBuilder, LinuxNamespace, LinuxNamespaceBuilder,
        LinuxResourcesBuilder, SpecBuilder,
    };
    use oci_spec::runtime::{LinuxNamespaceType, Root};
    use serial_test::serial;
    use tempfile::{tempdir, TempDir};
    use test_utils::{assert_result, skip_if_not_root};
    use ttrpc::{r#async::TtrpcContext, MessageHeader};

    const CGROUP_PARENT: &str = "kata.agent.test.k8s.io";

    fn mk_ttrpc_context() -> TtrpcContext {
        TtrpcContext {
            mh: MessageHeader::default(),
            metadata: std::collections::HashMap::new(),
            timeout_nano: 0,
        }
    }

    #[tokio::test]
    async fn minimal_device_rejections_precede_side_effects() {
        let sandbox = Sandbox::new(&slog::Logger::root(slog::Discard, o!())).unwrap();
        let service = AgentService {
            sandbox: Arc::new(Mutex::new(sandbox)),
        };
        let ctx = mk_ttrpc_context();
        let mut requests = Vec::new();
        for driver in [
            "mmioblk",
            "blk",
            "blk-ccw",
            "scsi",
            "nvdimm",
            "vfio-pci",
            "vfio-pci-gk",
            "vfio-ap",
            "unknown",
        ] {
            // No request may start waiting for a raw device, including MMIO.
            requests.push(protocols::agent::CreateContainerRequest {
                container_id: "device-rejection-test".into(),
                devices: vec![
                    protocols::agent::Device {
                        type_: "mmioblk".into(),
                        vm_path: "/dev/missing-canary-disk".into(),
                        container_path: "/dev/test".into(),
                        ..Default::default()
                    },
                    protocols::agent::Device {
                        type_: driver.into(),
                        ..Default::default()
                    },
                ],
                ..Default::default()
            });
        }
        requests.push(protocols::agent::CreateContainerRequest {
            devices: vec![protocols::agent::Device {
                type_: "mmioblk".into(),
                options: vec!["unsupported=true".into()],
                ..Default::default()
            }],
            ..Default::default()
        });
        requests.push(protocols::agent::CreateContainerRequest {
            shared_mounts: vec![Default::default()],
            ..Default::default()
        });
        let mut raw_spec = protocols::oci::Spec::default();
        raw_spec.Linux = MessageField::some(protocols::oci::Linux {
            Devices: vec![protocols::oci::LinuxDevice {
                Type: "b".into(),
                Path: "/dev/raw-disk".into(),
                Major: 8,
                Minor: 0,
                ..Default::default()
            }],
            ..Default::default()
        });
        requests.push(protocols::agent::CreateContainerRequest {
            OCI: MessageField::some(raw_spec),
            ..Default::default()
        });
        let mut spec = protocols::oci::Spec::default();
        spec.Annotations
            .insert("cdi.k8s.io/gpu".into(), "nvidia.com/gpu=all".into());
        requests.push(protocols::agent::CreateContainerRequest {
            OCI: MessageField::some(spec),
            ..Default::default()
        });
        let mut spec = protocols::oci::Spec::default();
        spec.Process = MessageField::some(protocols::oci::Process {
            Env: vec!["VISIBLE_CDI_DEVICES=nvidia.com/gpu=all".into()],
            ..Default::default()
        });
        requests.push(protocols::agent::CreateContainerRequest {
            OCI: MessageField::some(spec),
            ..Default::default()
        });
        // Hold the state lock: an invalid request must finish without acquiring
        // it, touching container state, or waiting for uevents.
        let state = service.sandbox.lock().await;
        for req in requests {
            let err = timeout(Duration::from_secs(1), service.create_container(&ctx, req))
                .await
                .expect("validation must precede sandbox operations")
                .unwrap_err();
            match err {
                ttrpc::Error::RpcStatus(status) => {
                    assert_eq!(status.code(), ttrpc::Code::INVALID_ARGUMENT)
                }
                error => panic!("unexpected error: {:?}", error),
            }
        }
        let req = protocols::agent::UpdateInterfaceRequest {
            interface: MessageField::some(Interface {
                devicePath: "00/01".into(),
                ..Default::default()
            }),
            ..Default::default()
        };
        match timeout(Duration::from_secs(1), service.update_interface(&ctx, req))
            .await
            .unwrap()
            .unwrap_err()
        {
            ttrpc::Error::RpcStatus(status) => {
                assert_eq!(status.code(), ttrpc::Code::INVALID_ARGUMENT)
            }
            error => panic!("unexpected error: {:?}", error),
        }
        assert!(state.containers.is_empty());
        assert!(state.uevent_map.is_empty());
        validate_container_device_features(&Default::default()).unwrap();
    }

    #[tokio::test]
    async fn minimal_removed_rpcs_are_unimplemented() {
        let sandbox = Sandbox::new(&slog::Logger::root(slog::Discard, o!())).unwrap();
        let service = AgentService {
            sandbox: Arc::new(Mutex::new(sandbox)),
        };
        let ctx = mk_ttrpc_context();
        macro_rules! reject {
            ($method:ident) => {
                match service.$method(&ctx, Default::default()).await.unwrap_err() {
                    ttrpc::Error::RpcStatus(status) => {
                        assert_eq!(status.code(), ttrpc::Code::UNIMPLEMENTED)
                    }
                    error => panic!("unexpected error: {:?}", error),
                }
            };
        }
        reject!(get_ip_tables);
        reject!(set_ip_tables);
        reject!(list_interfaces);
        reject!(list_routes);
        reject!(online_cpu_mem);
        reject!(update_ephemeral_mounts);
        reject!(set_guest_date_time);
        reject!(remove_stale_virtiofs_share_mounts);
        reject!(mem_agent_memcg_set);
        reject!(mem_agent_compact_set);
        reject!(mem_hotplug_by_probe);
        reject!(add_swap);
        reject!(add_swap_path);
        reject!(resize_volume);
        reject!(set_policy);
        reject!(reseed_random_dev);
        reject!(close_stdin);
    }

    #[tokio::test]
    async fn minimal_sandbox_rejects_extensions_before_side_effects() {
        let sandbox = Sandbox::new(&slog::Logger::root(slog::Discard, o!())).unwrap();
        let service = AgentService {
            sandbox: Arc::new(Mutex::new(sandbox)),
        };
        let ctx = mk_ttrpc_context();
        for req in [
            protocols::agent::CreateSandboxRequest {
                kernel_modules: vec![Default::default()],
                ..Default::default()
            },
            protocols::agent::CreateSandboxRequest {
                guest_hook_path: "/hooks".into(),
                ..Default::default()
            },
        ] {
            match service.create_sandbox(&ctx, req).await.unwrap_err() {
                ttrpc::Error::RpcStatus(status) => {
                    assert_eq!(status.code(), ttrpc::Code::INVALID_ARGUMENT)
                }
                error => panic!("unexpected error: {:?}", error),
            }
            assert!(!service.sandbox.lock().await.running);
        }
        validate_sandbox_features(&Default::default()).unwrap();
    }

    #[tokio::test]
    async fn minimal_services_reject_removed_io_and_storage_before_locking() {
        let sandbox = Sandbox::new(&slog::Logger::root(slog::Discard, o!())).unwrap();
        let service = AgentService {
            sandbox: Arc::new(Mutex::new(sandbox)),
        };
        let ctx = mk_ttrpc_context();
        let _guard = service.sandbox.lock().await;
        for ports in [(1, 0, 0), (0, 1026, 0), (0, 0, 1)] {
            let create = protocols::agent::CreateContainerRequest {
                stdin_port: ports.0,
                stdout_port: ports.1,
                stderr_port: ports.2,
                ..Default::default()
            };
            let exec = protocols::agent::ExecProcessRequest {
                stdin_port: ports.0,
                stdout_port: ports.1,
                stderr_port: ports.2,
                ..Default::default()
            };
            for result in [
                timeout(
                    Duration::from_secs(1),
                    service.create_container(&ctx, create),
                )
                .await
                .unwrap(),
                timeout(Duration::from_secs(1), service.exec_process(&ctx, exec))
                    .await
                    .unwrap(),
            ] {
                match result.unwrap_err() {
                    ttrpc::Error::RpcStatus(status) => {
                        assert_eq!(status.code(), ttrpc::Code::INVALID_ARGUMENT)
                    }
                    e => panic!("unexpected error: {:?}", e),
                }
            }
        }
        for storage in [
            protocols::agent::Storage {
                driver: "image_guest_pull".into(),
                ..Default::default()
            },
            protocols::agent::Storage {
                driver: "mmioblk".into(),
                driver_options: vec!["encryption_key=ephemeral".into()],
                ..Default::default()
            },
            protocols::agent::Storage {
                driver: "mmioblk".into(),
                driver_options: vec!["unknown=true".into()],
                ..Default::default()
            },
        ] {
            // A valid first item must not be mounted before the invalid second is checked.
            let storages = vec![
                protocols::agent::Storage {
                    driver: "local".into(),
                    ..Default::default()
                },
                storage,
            ];
            let create = protocols::agent::CreateContainerRequest {
                storages: storages.clone(),
                ..Default::default()
            };
            let sandbox = protocols::agent::CreateSandboxRequest {
                storages,
                ..Default::default()
            };
            for result in [
                timeout(
                    Duration::from_secs(1),
                    service.create_container(&ctx, create),
                )
                .await
                .unwrap(),
                timeout(
                    Duration::from_secs(1),
                    service.create_sandbox(&ctx, sandbox),
                )
                .await
                .unwrap(),
            ] {
                match result.unwrap_err() {
                    ttrpc::Error::RpcStatus(status) => {
                        assert_eq!(status.code(), ttrpc::Code::INVALID_ARGUMENT)
                    }
                    e => panic!("unexpected error: {:?}", e),
                }
            }
        }
    }

    #[tokio::test]
    async fn minimal_guest_details_reject_hotplug_and_keep_agent_metadata() {
        let service = AgentService {
            sandbox: Arc::new(Mutex::new(Sandbox::new(&sl()).unwrap())),
        };
        let ctx = mk_ttrpc_context();
        let _guard = service.sandbox.lock().await;
        for (block, probe) in [(true, false), (false, true), (true, true)] {
            let request = protocols::agent::GuestDetailsRequest {
                mem_block_size: block,
                mem_hotplug_probe: probe,
                ..Default::default()
            };
            match service.get_guest_details(&ctx, request).await.unwrap_err() {
                ttrpc::Error::RpcStatus(status) => {
                    assert_eq!(status.code(), ttrpc::Code::INVALID_ARGUMENT)
                }
                e => panic!("unexpected error: {:?}", e),
            }
        }
        let response = service
            .get_guest_details(&ctx, Default::default())
            .await
            .unwrap();
        assert_eq!(response.mem_block_size_bytes, 0);
        assert!(!response.support_mem_hotplug_probe);
        let details = response.agent_details.unwrap();
        assert_eq!(details.version, AGENT_VERSION);
        assert!(details.device_handlers.is_empty());
        assert!(details
            .storage_handlers
            .iter()
            .any(|driver| driver == "mmioblk"));
    }

    fn create_dummy_opts() -> CreateOpts {
        let start = SystemTime::now();
        let since_the_epoch = start
            .duration_since(UNIX_EPOCH)
            .expect("Time went backwards");

        let mut root = Root::default();
        root.set_path(PathBuf::from("/"));

        let linux_resources = LinuxResourcesBuilder::default()
            .devices(vec![LinuxDeviceCgroupBuilder::default()
                .allow(true)
                .access("rwm")
                .build()
                .unwrap()])
            .build()
            .unwrap();

        let cgroups_path = format!(
            "/{}/dummycontainer{}",
            CGROUP_PARENT,
            since_the_epoch.as_micros()
        );

        let spec = SpecBuilder::default()
            .linux(
                LinuxBuilder::default()
                    .cgroups_path(cgroups_path)
                    .resources(linux_resources)
                    .build()
                    .unwrap(),
            )
            .root(root)
            .build()
            .unwrap();

        CreateOpts {
            cgroup_name: "".to_string(),
            use_systemd_cgroup: false,
            no_pivot_root: false,
            no_new_keyring: false,
            spec: Some(spec),
            rootless_euid: false,
            rootless_cgroup: false,
            container_name: "".to_string(),
        }
    }

    fn create_linuxcontainer() -> (LinuxContainer, TempDir) {
        let dir = tempdir().expect("failed to make tempdir");

        (
            LinuxContainer::new(
                "some_id",
                dir.path().join("rootfs").to_str().unwrap(),
                None,
                create_dummy_opts(),
                &slog_scope::logger(),
            )
            .unwrap(),
            dir,
        )
    }

    #[tokio::test]
    async fn test_update_interface() {
        let logger = slog::Logger::root(slog::Discard, o!());
        let sandbox = Sandbox::new(&logger).unwrap();

        let agent_service = Box::new(AgentService {
            sandbox: Arc::new(Mutex::new(sandbox)),
        });

        let req = protocols::agent::UpdateInterfaceRequest::default();
        let ctx = mk_ttrpc_context();

        let result = agent_service.update_interface(&ctx, req).await;

        assert!(result.is_err(), "expected update interface to fail");
    }

    #[tokio::test]
    async fn test_update_routes() {
        let logger = slog::Logger::root(slog::Discard, o!());
        let sandbox = Sandbox::new(&logger).unwrap();
        let agent_service = Box::new(AgentService {
            sandbox: Arc::new(Mutex::new(sandbox)),
        });

        let req = protocols::agent::UpdateRoutesRequest::default();
        let ctx = mk_ttrpc_context();

        let result = agent_service.update_routes(&ctx, req).await;

        assert!(result.is_err(), "expected update routes to fail");
    }

    #[tokio::test]
    async fn test_add_arp_neighbors() {
        let logger = slog::Logger::root(slog::Discard, o!());
        let sandbox = Sandbox::new(&logger).unwrap();
        let agent_service = Box::new(AgentService {
            sandbox: Arc::new(Mutex::new(sandbox)),
        });

        let req = protocols::agent::AddARPNeighborsRequest::default();
        let ctx = mk_ttrpc_context();

        let result = agent_service.add_arp_neighbors(&ctx, req).await;

        assert!(result.is_err(), "expected add arp neighbors to fail");
    }

    #[tokio::test]
    #[serial]
    async fn test_do_write_stream() {
        skip_if_not_root!();

        #[derive(Debug)]
        struct TestData<'a> {
            create_container: bool,
            has_fd: bool,
            has_tty: bool,
            break_pipe: bool,

            container_id: &'a str,
            exec_id: &'a str,
            data: Vec<u8>,
            result: Result<protocols::agent::WriteStreamResponse>,
        }

        impl Default for TestData<'_> {
            fn default() -> Self {
                TestData {
                    create_container: true,
                    has_fd: true,
                    has_tty: true,
                    break_pipe: false,

                    container_id: "1",
                    exec_id: "2",
                    data: vec![1, 2, 3],
                    result: Ok(WriteStreamResponse {
                        len: 3,
                        ..WriteStreamResponse::default()
                    }),
                }
            }
        }

        let tests = &[
            TestData {
                ..Default::default()
            },
            TestData {
                has_tty: false,
                ..Default::default()
            },
            TestData {
                break_pipe: true,
                result: Err(anyhow!(std::io::Error::from_raw_os_error(libc::EPIPE))),
                ..Default::default()
            },
            TestData {
                create_container: false,
                result: Err(anyhow!(crate::sandbox::SandboxError::InvalidContainerId)),
                ..Default::default()
            },
            TestData {
                container_id: "8181",
                result: Err(anyhow!(crate::sandbox::SandboxError::InvalidContainerId)),
                ..Default::default()
            },
            TestData {
                data: vec![],
                result: Ok(WriteStreamResponse {
                    len: 0,
                    ..WriteStreamResponse::default()
                }),
                ..Default::default()
            },
        ];

        for (i, d) in tests.iter().enumerate() {
            let msg = format!("test[{i}]: {d:?}");

            let logger = slog::Logger::root(slog::Discard, o!());
            let mut sandbox = Sandbox::new(&logger).unwrap();

            let (rfd, wfd) = unistd::pipe().unwrap();
            let rfd = if d.break_pipe {
                drop(rfd); // OwnedFd closes automatically on drop
                None
            } else {
                Some(rfd)
            };

            if d.create_container {
                let (mut linux_container, _root) = create_linuxcontainer();
                let exec_process_id = 2;

                linux_container.id = "1".to_string();

                let mut exec_process = Process::new(
                    &logger,
                    &oci::Process::default(),
                    &exec_process_id.to_string(),
                    false,
                    1,
                )
                .unwrap();

                let fd = if d.has_fd {
                    let raw_fd = wfd.as_raw_fd();
                    std::mem::forget(wfd); // Prevent OwnedFd from closing the fd
                    Some(raw_fd)
                } else {
                    // Let wfd drop naturally to close the fd
                    drop(wfd);
                    None
                };

                if d.has_tty {
                    exec_process.parent_stdin = None;
                    exec_process.term_master = fd;
                } else {
                    exec_process.parent_stdin = fd;
                    exec_process.term_master = None;
                }
                linux_container
                    .processes
                    .insert(exec_process.exec_id.clone(), exec_process);

                sandbox.add_container(linux_container);
            }

            let agent_service = Box::new(AgentService {
                sandbox: Arc::new(Mutex::new(sandbox)),
            });

            let result = agent_service
                .do_write_stream(protocols::agent::WriteStreamRequest {
                    container_id: d.container_id.to_string(),
                    exec_id: d.exec_id.to_string(),
                    data: d.data.clone(),
                    ..Default::default()
                })
                .await;

            drop(rfd);
            // XXX: Do not close wfd.
            // the fd will be closed on Process's dropping.
            // unistd::close(wfd).unwrap();

            let msg = format!("{msg}, result: {result:?}");
            assert_result!(d.result, result, msg);
        }
    }
    #[tokio::test]
    async fn test_update_container_namespaces() {
        #[derive(Debug)]
        struct TestData<'a> {
            has_linux_in_spec: bool,
            sandbox_pidns_path: Option<&'a str>,

            namespaces: Vec<LinuxNamespace>,
            use_sandbox_pidns: bool,
            result: Result<()>,
            expected_namespaces: Vec<LinuxNamespace>,
        }

        impl Default for TestData<'_> {
            fn default() -> Self {
                TestData {
                    has_linux_in_spec: true,
                    sandbox_pidns_path: Some("sharedpidns"),
                    namespaces: vec![
                        LinuxNamespaceBuilder::default()
                            .typ(LinuxNamespaceType::Ipc)
                            .path("ipcpath")
                            .build()
                            .unwrap(),
                        LinuxNamespaceBuilder::default()
                            .typ(LinuxNamespaceType::Uts)
                            .path("utspath")
                            .build()
                            .unwrap(),
                    ],
                    use_sandbox_pidns: false,
                    result: Ok(()),
                    expected_namespaces: vec![
                        LinuxNamespaceBuilder::default()
                            .typ(LinuxNamespaceType::Ipc)
                            .build()
                            .unwrap(),
                        LinuxNamespaceBuilder::default()
                            .typ(LinuxNamespaceType::Uts)
                            .build()
                            .unwrap(),
                        LinuxNamespaceBuilder::default()
                            .typ(LinuxNamespaceType::Pid)
                            .build()
                            .unwrap(),
                    ],
                }
            }
        }

        let tests = &[
            TestData {
                ..Default::default()
            },
            TestData {
                use_sandbox_pidns: true,
                expected_namespaces: vec![
                    LinuxNamespaceBuilder::default()
                        .typ(LinuxNamespaceType::Ipc)
                        .build()
                        .unwrap(),
                    LinuxNamespaceBuilder::default()
                        .typ(LinuxNamespaceType::Uts)
                        .build()
                        .unwrap(),
                    LinuxNamespaceBuilder::default()
                        .typ(LinuxNamespaceType::Pid)
                        .path("sharedpidns")
                        .build()
                        .unwrap(),
                ],
                ..Default::default()
            },
            TestData {
                namespaces: vec![],
                use_sandbox_pidns: true,
                expected_namespaces: vec![LinuxNamespaceBuilder::default()
                    .typ(LinuxNamespaceType::Pid)
                    .path("sharedpidns")
                    .build()
                    .unwrap()],
                ..Default::default()
            },
            TestData {
                namespaces: vec![],
                use_sandbox_pidns: false,
                expected_namespaces: vec![LinuxNamespaceBuilder::default()
                    .typ(LinuxNamespaceType::Pid)
                    .build()
                    .unwrap()],
                ..Default::default()
            },
            TestData {
                has_linux_in_spec: false,
                result: Err(anyhow!(ERR_NO_LINUX_FIELD)),
                ..Default::default()
            },
        ];

        for (i, d) in tests.iter().enumerate() {
            let msg = format!("test[{i}]: {d:?}");

            let logger = slog::Logger::root(slog::Discard, o!());
            let mut sandbox = Sandbox::new(&logger).unwrap();
            if let Some(pidns_path) = d.sandbox_pidns_path {
                let mut sandbox_pidns = Namespace::new(&logger);
                sandbox_pidns.path = pidns_path.to_string();
                sandbox.sandbox_pidns = Some(sandbox_pidns);
            }

            let mut oci = Spec::default();
            oci.set_linux(None);
            if d.has_linux_in_spec {
                let mut linux = Linux::default();
                linux.set_namespaces(Some(d.namespaces.clone()));
                oci.set_linux(Some(linux));
            }

            let result = update_container_namespaces(&sandbox, &mut oci, d.use_sandbox_pidns);

            let msg = format!("{msg}, result: {result:?}");

            assert_result!(d.result, result, msg);
            if let Some(linux) = oci.linux() {
                assert_eq!(
                    d.expected_namespaces,
                    linux.namespaces().clone().unwrap(),
                    "{msg}"
                );
            }
        }
    }

    #[tokio::test]
    async fn test_is_signal_handled() {
        #[derive(Debug)]
        struct TestData<'a> {
            status_file_data: Option<&'a str>,
            signum: u32,
            result: bool,
        }

        let tests = &[
            TestData {
                status_file_data: Some(
                    r#"
SigBlk:0000000000010000
SigCgt:0000000000000001
OtherField:other
                "#,
                ),
                signum: 1,
                result: true,
            },
            TestData {
                status_file_data: Some("SigCgt:000000004b813efb"),
                signum: 4,
                result: true,
            },
            TestData {
                status_file_data: Some("SigCgt:\t000000004b813efb"),
                signum: 4,
                result: true,
            },
            TestData {
                status_file_data: Some("SigCgt: 000000004b813efb"),
                signum: 4,
                result: true,
            },
            TestData {
                status_file_data: Some("SigCgt:000000004b813efb "),
                signum: 4,
                result: true,
            },
            TestData {
                status_file_data: Some("SigCgt:\t000000004b813efb "),
                signum: 4,
                result: true,
            },
            TestData {
                status_file_data: Some("SigCgt:000000004b813efb"),
                signum: 3,
                result: false,
            },
            TestData {
                status_file_data: Some("SigCgt:000000004b813efb"),
                signum: 65,
                result: false,
            },
            TestData {
                status_file_data: Some("SigCgt:000000004b813efb"),
                signum: 0,
                result: true,
            },
            TestData {
                status_file_data: Some("SigCgt:ZZZZZZZZ"),
                signum: 1,
                result: false,
            },
            TestData {
                status_file_data: Some("SigCgt:-1"),
                signum: 1,
                result: false,
            },
            TestData {
                status_file_data: Some("SigCgt"),
                signum: 1,
                result: false,
            },
            TestData {
                status_file_data: Some("any data"),
                signum: 0,
                result: true,
            },
            TestData {
                status_file_data: Some("SigBlk:0000000000000001"),
                signum: 1,
                result: true,
            },
            TestData {
                status_file_data: Some("SigIgn:0000000000000001"),
                signum: 1,
                result: true,
            },
            TestData {
                status_file_data: None,
                signum: 1,
                result: false,
            },
            TestData {
                status_file_data: None,
                signum: 0,
                result: false,
            },
        ];

        for (i, d) in tests.iter().enumerate() {
            let msg = format!("test[{i}]: {d:?}");

            let dir = tempdir().expect("failed to make tempdir");
            let proc_status_file_path = dir.path().join("status");

            if let Some(file_data) = d.status_file_data {
                fs::write(&proc_status_file_path, file_data).unwrap();
            }

            let result = is_signal_handled(proc_status_file_path.to_str().unwrap(), d.signum);

            let msg = format!("{msg}, result: {result:?}");

            assert_eq!(d.result, result, "{msg}");
        }
    }

    #[tokio::test]
    async fn test_volume_capacity_stats() {
        skip_if_not_root!();

        // Verify error if path does not exist
        assert!(get_volume_capacity_stats("/does-not-exist").is_err());

        // Create a new tmpfs mount, and verify the initial values
        let mount_dir = tempfile::tempdir().unwrap();
        mount::mount(
            Some("tmpfs"),
            mount_dir.path().to_str().unwrap(),
            Some("tmpfs"),
            mount::MsFlags::empty(),
            None::<&str>,
        )
        .unwrap();
        let mut stats = get_volume_capacity_stats(mount_dir.path().to_str().unwrap()).unwrap();
        assert_eq!(stats.used, 0);
        assert_ne!(stats.available, 0);
        let available = stats.available;

        // Verify that writing a file will result in increased utilization
        fs::write(mount_dir.path().join("file.dat"), "foobar").unwrap();
        stats = get_volume_capacity_stats(mount_dir.path().to_str().unwrap()).unwrap();

        let size = get_block_size(mount_dir.path().to_str().unwrap()).unwrap();

        assert_eq!(stats.used, size);
        assert_eq!(stats.available, available - size);
    }

    fn get_block_size(path: &str) -> Result<u64, Errno> {
        let stat = statfs::statfs(path)?;
        let block_size = stat.block_size() as u64;
        Ok(block_size)
    }

    #[tokio::test]
    async fn test_get_volume_inode_stats() {
        skip_if_not_root!();

        // Verify error if path does not exist
        assert!(get_volume_inode_stats("/does-not-exist").is_err());

        // Create a new tmpfs mount, and verify the initial values
        let mount_dir = tempfile::tempdir().unwrap();
        mount::mount(
            Some("tmpfs"),
            mount_dir.path().to_str().unwrap(),
            Some("tmpfs"),
            mount::MsFlags::empty(),
            None::<&str>,
        )
        .unwrap();
        let mut stats = get_volume_inode_stats(mount_dir.path().to_str().unwrap()).unwrap();
        assert_eq!(stats.used, 1);
        assert_ne!(stats.available, 0);
        let available = stats.available;

        // Verify that creating a directory and writing a file will result in increased utilization
        let dir = mount_dir.path().join("foobar");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.as_path().join("file.dat"), "foobar").unwrap();
        stats = get_volume_inode_stats(mount_dir.path().to_str().unwrap()).unwrap();

        assert_eq!(stats.used, 3);
        assert_eq!(stats.available, available - 2);
    }

    #[tokio::test]
    async fn test_get_oom_event_no_deadlock() {
        let logger = slog::Logger::root(slog::Discard, o!());
        let sandbox = Sandbox::new(&logger).unwrap();

        let agent_service = Arc::new(AgentService {
            sandbox: Arc::new(Mutex::new(sandbox)),
        });

        let svc1 = agent_service.clone();
        let handle1 = tokio::spawn(async move {
            let ctx = mk_ttrpc_context();
            let req = protocols::agent::GetOOMEventRequest::default();
            svc1.get_oom_event(&ctx, req).await
        });

        // Yield until handler #1 has released the sandbox lock (entered recv()).
        // Each yield_now() gives the spawned task a chance to make progress.
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                tokio::task::yield_now().await;
                if agent_service.sandbox.try_lock().is_ok() {
                    return;
                }
            }
        })
        .await
        .expect("sandbox lock should be free while get_oom_event waits");

        let svc2 = agent_service.clone();
        let handle2 = tokio::spawn(async move {
            let ctx = mk_ttrpc_context();
            let req = protocols::agent::GetOOMEventRequest::default();
            svc2.get_oom_event(&ctx, req).await
        });

        // Yield until handler #2 has also released the sandbox lock (entered recv()).
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                tokio::task::yield_now().await;
                if agent_service.sandbox.try_lock().is_ok() {
                    return;
                }
            }
        })
        .await
        .expect("sandbox lock should be free with two concurrent get_oom_event handlers");

        let tx = {
            let s = agent_service.sandbox.lock().await;
            s.event_tx.as_ref().unwrap().clone()
        };
        tx.send("container-1".to_string()).await.unwrap();
        tx.send("container-2".to_string()).await.unwrap();

        let result1 = tokio::time::timeout(std::time::Duration::from_secs(5), handle1).await;
        let result2 = tokio::time::timeout(std::time::Duration::from_secs(5), handle2).await;

        assert!(result1.is_ok(), "handler #1 timed out — possible deadlock");
        assert!(result2.is_ok(), "handler #2 timed out — possible deadlock");

        let resp1 = result1.unwrap().unwrap().unwrap();
        let resp2 = result2.unwrap().unwrap().unwrap();

        let mut ids: Vec<String> = vec![resp1.container_id, resp2.container_id];
        ids.sort();
        assert_eq!(ids, vec!["container-1", "container-2"]);
    }

    #[test]
    fn test_do_copy_file_requires_existing_parents() {
        let temp_dir = tempdir().unwrap();
        let base = temp_dir.path().join("shared");
        for file_mode in [libc::S_IFREG, libc::S_IFDIR, libc::S_IFLNK] {
            let mut req = CopyFileRequest {
                path: base.join("target").to_string_lossy().into(),
                file_mode: file_mode | 0o755,
                data: b"destination".to_vec(),
                file_size: 11,
                uid: unistd::getuid().as_raw() as i32,
                gid: unistd::getgid().as_raw() as i32,
                ..Default::default()
            };
            // CopyFile must not bootstrap even the fixed shared root.
            assert!(do_copy_file(&req, &base).is_err());
            assert!(!base.exists());
            fs::create_dir(&base).unwrap();
            req.path = base.join("missing/nested/target").to_string_lossy().into();
            assert!(do_copy_file(&req, &base).is_err());
            assert!(!base.join("missing").exists());
            assert_eq!(fs::read_dir(&base).unwrap().count(), 0);
            fs::remove_dir(&base).unwrap();
        }
    }

    #[test]
    fn test_do_copy_file_preserves_parent_metadata() {
        let temp_dir = tempdir().unwrap();
        let base = temp_dir.path().to_path_buf();
        let parent = base.join("parent");
        fs::create_dir(&parent).unwrap();
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o751)).unwrap();
        // Exercise a different parent owner when run as root, as in the Linux builder.
        if unistd::getuid().is_root() {
            unistd::chown(
                &parent,
                Some(Uid::from_raw(1234)),
                Some(Gid::from_raw(1234)),
            )
            .unwrap();
        }
        let before = stat::stat(&parent).unwrap();
        for (name, kind) in [
            ("file", libc::S_IFREG),
            ("dir", libc::S_IFDIR),
            ("link", libc::S_IFLNK),
        ] {
            let req = CopyFileRequest {
                path: parent.join(name).to_string_lossy().into(),
                file_mode: kind | 0o700,
                dir_mode: 0o777,
                uid: unistd::getuid().as_raw() as i32,
                gid: unistd::getgid().as_raw() as i32,
                data: b"destination".to_vec(),
                file_size: 11,
                ..Default::default()
            };
            do_copy_file(&req, &base).unwrap();
            let after = stat::stat(&parent).unwrap();
            assert_eq!(
                (after.st_uid, after.st_gid, after.st_mode),
                (before.st_uid, before.st_gid, before.st_mode)
            );
            let target = stat::lstat(&parent.join(name)).unwrap();
            assert_eq!(
                (target.st_uid, target.st_gid),
                (req.uid as u32, req.gid as u32)
            );
        }
    }

    #[tokio::test]
    async fn test_do_copy_file() {
        let temp_dir = tempdir().expect("creating temp dir failed");
        let base = temp_dir.path().join("shared");
        fs::create_dir(&base).unwrap();
        for parent in ["a", "x"] {
            fs::create_dir(base.join(parent)).unwrap();
            fs::set_permissions(base.join(parent), fs::Permissions::from_mode(0o755)).unwrap();
        }

        type Assertions = Box<dyn Fn(&Path) -> Result<()>>;
        struct TestCase {
            name: String,
            request: CopyFileRequest,
            assertions: Assertions,
            should_fail: bool,
        }

        // Attention: these test cases depend on each other and can't be reordered.
        // The first few cases build up a directory structure that the subsequent tests then rely
        // on or try to exploit.
        // TODO(burgerdev): define a common  directory structure for all tests up front.
        let tests = [
            TestCase {
                name: "Create a top-level file".into(),
                request: CopyFileRequest {
                    path: base.join("f").to_string_lossy().into(),
                    file_mode: 0o644 | libc::S_IFREG,
                    ..Default::default()
                },
                should_fail: false,
                assertions: Box::new(|base| -> Result<()> {
                    let f = base.join("f");
                    let f_stat = fs::metadata(&f).context("stat ./f failed")?;
                    ensure!(f_stat.is_file());
                    ensure!(0o644 == f_stat.permissions().mode() & 0o777);
                    let content = std::fs::read_to_string(&f).context("read ./f failed")?;
                    ensure!(content.is_empty());
                    Ok(())
                }),
            },
            TestCase {
                name: "Writing a file onto an existing file replaces it".into(),
                request: CopyFileRequest {
                    path: base.join("f").to_string_lossy().into(),
                    file_mode: 0o600 | libc::S_IFREG,
                    data: b"Hello!".to_vec(),
                    file_size: 6,
                    ..Default::default()
                },
                should_fail: false,
                assertions: Box::new(|base| -> Result<()> {
                    let f = base.join("f");
                    let f_stat = fs::metadata(&f).context("stat ./f failed")?;
                    ensure!(f_stat.is_file());
                    ensure!(0o600 == f_stat.permissions().mode() & 0o777);
                    let content = std::fs::read_to_string(&f).context("read ./f failed")?;
                    ensure!("Hello!" == content);
                    Ok(())
                }),
            },
            TestCase {
                name: "Creating a file preserves its existing parent".into(),
                request: CopyFileRequest {
                    path: base.join("a/b").to_string_lossy().into(),
                    dir_mode: 0o755 | libc::S_IFDIR,
                    file_mode: 0o644 | libc::S_IFREG,
                    ..Default::default()
                },
                should_fail: false,
                assertions: Box::new(|base| -> Result<()> {
                    let a_stat = fs::metadata(base.join("a")).context("stat ./a failed")?;
                    ensure!(a_stat.is_dir());
                    ensure!(0o755 == a_stat.permissions().mode() & 0o777);
                    let b_stat = fs::metadata(base.join("a/b")).context("stat ./a/b failed")?;
                    ensure!(b_stat.is_file());
                    ensure!(0o644 == b_stat.permissions().mode() & 0o777);
                    Ok(())
                }),
            },
            TestCase {
                name: "Create a file within an existing directory".into(),
                request: CopyFileRequest {
                    path: base.join("a/c").to_string_lossy().into(),
                    dir_mode: 0o700 | libc::S_IFDIR, // Test that existing directories are not touched - we expect this to stay 0o755.
                    file_mode: 0o621 | libc::S_IFREG,
                    ..Default::default()
                },
                should_fail: false,
                assertions: Box::new(|base| -> Result<()> {
                    let a_stat = fs::metadata(base.join("a")).context("stat ./a failed")?;
                    ensure!(a_stat.is_dir());
                    ensure!(0o755 == a_stat.permissions().mode() & 0o777);
                    let c_stat = fs::metadata(base.join("a/c")).context("stat ./a/c failed")?;
                    ensure!(c_stat.is_file());
                    ensure!(0o621 == c_stat.permissions().mode() & 0o777);
                    Ok(())
                }),
            },
            TestCase {
                name: "Create a directory".into(),
                request: CopyFileRequest {
                    path: base.join("a/d").to_string_lossy().into(),
                    dir_mode: 0o700 | libc::S_IFDIR, // Test that the permissions are taken from file_mode.
                    file_mode: 0o755 | libc::S_IFDIR,
                    ..Default::default()
                },
                should_fail: false,
                assertions: Box::new(|base| -> Result<()> {
                    let a_stat = fs::metadata(base.join("a")).context("stat ./a failed")?;
                    ensure!(a_stat.is_dir());
                    ensure!(0o755 == a_stat.permissions().mode() & 0o777);
                    let d_stat = fs::metadata(base.join("a/d")).context("stat ./a/d failed")?;
                    ensure!(d_stat.is_dir());
                    ensure!(0o755 == d_stat.permissions().mode() & 0o777);
                    Ok(())
                }),
            },
            TestCase {
                name: "Creating a dir onto an existing file replaces the file".into(),
                request: CopyFileRequest {
                    path: base.join("a/b").to_string_lossy().into(),
                    dir_mode: 0o700 | libc::S_IFDIR, // Test that the permissions are taken from file_mode.
                    file_mode: 0o755 | libc::S_IFDIR,
                    ..Default::default()
                },
                should_fail: false,
                assertions: Box::new(|base| -> Result<()> {
                    let b_stat = fs::metadata(base.join("a/b")).context("stat ./a/b failed")?;
                    ensure!(b_stat.is_dir());
                    ensure!(0o755 == b_stat.permissions().mode() & 0o777);
                    Ok(())
                }),
            },
            TestCase {
                name: "Creating a file onto an existing dir replaces the dir".into(),
                request: CopyFileRequest {
                    path: base.join("a/b").to_string_lossy().into(),
                    dir_mode: 0o755 | libc::S_IFDIR,
                    file_mode: 0o644 | libc::S_IFREG,
                    ..Default::default()
                },
                should_fail: false,
                assertions: Box::new(|base| -> Result<()> {
                    let b_stat = fs::metadata(base.join("a/b")).context("stat ./a/b failed")?;
                    ensure!(b_stat.is_file());
                    ensure!(0o644 == b_stat.permissions().mode() & 0o777);
                    Ok(())
                }),
            },
            TestCase {
                name: "Creating a dir onto an existing dir does not replace that dir".into(),
                request: CopyFileRequest {
                    path: base.join("a").to_string_lossy().into(),
                    dir_mode: 0o755 | libc::S_IFDIR,
                    file_mode: 0o751 | libc::S_IFDIR,
                    ..Default::default()
                },
                should_fail: false,
                assertions: Box::new(|base| -> Result<()> {
                    // Check that a/b still exists
                    let b_stat = fs::metadata(base.join("a/b")).context("stat ./a/b failed")?;
                    ensure!(b_stat.is_file());
                    let a_stat = fs::metadata(base.join("a")).context("stat ./a failed")?;
                    ensure!(0o751 == a_stat.permissions().mode() & 0o777);
                    Ok(())
                }),
            },
            TestCase {
                name: "Create a symlink".into(),
                request: CopyFileRequest {
                    path: base.join("a/link").to_string_lossy().into(),
                    dir_mode: 0o700 | libc::S_IFDIR, // Test that the permissions are taken from file_mode.
                    file_mode: 0o755 | libc::S_IFLNK,
                    data: b"/etc/passwd".to_vec(),
                    ..Default::default()
                },
                should_fail: false,
                assertions: Box::new(|base| -> Result<()> {
                    let link = base.join("a/link");
                    let link_stat = nix::sys::stat::lstat(&link).context("stat ./a/link failed")?;
                    // Linux symlinks have no permissions!
                    ensure!(0o777 | libc::S_IFLNK == link_stat.st_mode);
                    let target = fs::read_link(&link).context("read_link ./a/link failed")?;
                    ensure!(target.to_string_lossy() == "/etc/passwd");
                    Ok(())
                }),
            },
            TestCase {
                name: "Create a directory with setgid and sticky bit".into(),
                request: CopyFileRequest {
                    path: base.join("x/y").to_string_lossy().into(),
                    dir_mode: 0o3755 | libc::S_IFDIR,
                    file_mode: 0o3770 | libc::S_IFDIR,
                    ..Default::default()
                },
                should_fail: false,
                assertions: Box::new(|base| -> Result<()> {
                    // Existing parents must retain their permissions.
                    let x_stat = fs::metadata(base.join("x")).context("stat ./x failed")?;
                    ensure!(x_stat.is_dir());
                    ensure!(0o755 == x_stat.permissions().mode() & 0o7777);
                    // Explicitly created directories should.
                    let y_stat = fs::metadata(base.join("x/y")).context("stat ./x/y failed")?;
                    ensure!(y_stat.is_dir());
                    ensure!(0o3770 == y_stat.permissions().mode() & 0o7777);
                    Ok(())
                }),
            },
            TestCase {
                name: "Chunked upload 1".into(),
                request: CopyFileRequest {
                    path: base.join("x/chunked").to_string_lossy().into(),
                    dir_mode: 0o755 | libc::S_IFDIR,
                    file_mode: 0o644 | libc::S_IFREG,
                    offset: 0,
                    file_size: 11,
                    data: b"Hello ".to_vec(),
                    ..Default::default()
                },
                should_fail: false,
                assertions: Box::new(|base| -> Result<()> {
                    ensure!(
                        !(fs::exists(base.join("x/chunked"))
                            .context("exists ./x/chunked failed")?)
                    );
                    Ok(())
                }),
            },
            TestCase {
                name: "Chunked upload 2".into(),
                request: CopyFileRequest {
                    path: base.join("x/chunked").to_string_lossy().into(),
                    dir_mode: 0o755 | libc::S_IFDIR,
                    file_mode: 0o644 | libc::S_IFREG,
                    offset: 6,
                    file_size: 11,
                    data: b"World".to_vec(),
                    ..Default::default()
                },
                should_fail: false,
                assertions: Box::new(|base| -> Result<()> {
                    let content = std::fs::read(base.join("x/chunked"))?;
                    println!("{:?}", content);
                    ensure!(b"Hello World".to_vec() == content);
                    Ok(())
                }),
            },
            // =================================
            // Below are some adversarial tests.
            // =================================
            TestCase {
                name: "Malicious intermediate directory is a symlink".into(),
                request: CopyFileRequest {
                    path: base
                        .join("a/link/this-could-just-be-shadow-but-I-am-not-risking-it")
                        .to_string_lossy()
                        .into(),
                    dir_mode: 0o700 | libc::S_IFDIR, // Test that the permissions are taken from file_mode.
                    file_mode: 0o755 | libc::S_IFLNK,
                    data: b"root:password:19000:0:99999:7:::\n".to_vec(),
                    file_size: 33,
                    ..Default::default()
                },
                should_fail: true,
                assertions: Box::new(|base| -> Result<()> {
                    let link_stat = nix::sys::stat::lstat(&base.join("a/link"))
                        .context("stat ./a/link failed")?;
                    ensure!(0o777 | libc::S_IFLNK == link_stat.st_mode);
                    Ok(())
                }),
            },
            TestCase {
                name: "Creating a symlink onto an existing symlink should replace the symlink, not follow it".into(),
                request: CopyFileRequest {
                    path: base.join("a/link").to_string_lossy().into(),
                    dir_mode: 0o700 | libc::S_IFDIR, // Test that the permissions are taken from file_mode.
                    file_mode: 0o755 | libc::S_IFLNK,
                    data: b"/etc".to_vec(),
                    ..Default::default()
                },
                should_fail: false,
                assertions: Box::new(|base| -> Result<()> {
                    // The symlink should be created at the same place (not followed), with the new content.
                    let a_stat = fs::metadata(base.join("a")).context("stat ./a failed")?;
                    ensure!(a_stat.is_dir());
                    ensure!(0o751 == a_stat.permissions().mode() & 0o777);
                    let link = base.join("a/link");
                    let link_stat = nix::sys::stat::lstat(&link).context("stat ./a/link failed")?;
                    // Linux symlinks have no permissions!
                    ensure!(0o777 | libc::S_IFLNK == link_stat.st_mode);
                    let target = fs::read_link(&link).context("read_link ./a/link failed")?;
                    ensure!(target.to_string_lossy() == "/etc");
                    Ok(())
                }),
            },
            TestCase {
                name: "Creating a file at an existing symlink replaces the link and does not follow it".into(),
                request: CopyFileRequest {
                    path: base.join("a/link").to_string_lossy().into(),
                    file_mode: 0o600 | libc::S_IFREG,
                    data: b"Hello!".to_vec(),
                    file_size: 6,
                    ..Default::default()
                },
                should_fail: false,
                assertions: Box::new(|base| -> Result<()> {
                    // The symlink itself should be replaced with the file, not followed.
                    let link = base.join("a/link");
                    let link_stat = nix::sys::stat::lstat(&link).context("stat ./a/link failed")?;
                    ensure!(0o600 | libc::S_IFREG == link_stat.st_mode);
                    let content = std::fs::read_to_string(&link).context("read ./a/link failed")?;
                    ensure!("Hello!" == content);
                    Ok(())
                }),
            },
            TestCase {
                name: "Writing outside the shared directory is rejected".into(),
                request: CopyFileRequest {
                    path: base.parent().unwrap().join("not-shared").to_string_lossy().into(),
                    file_mode: 0o600 | libc::S_IFREG,
                    ..Default::default()
                },
                should_fail: true,
                assertions: Box::new(|base| -> Result<()> {
                    match fs::metadata(base.parent().unwrap().join("not-shared")) {
                        Ok(_) => bail!("successful write outside shared directory"),
                        Err(_) => Ok(())
                    }
                }),
            },
            TestCase {
                name: "Traversal outside shared directory is rejected".into(),
                request: CopyFileRequest {
                    path: base.join("../not-shared").to_string_lossy().into(),
                    file_mode: 0o600 | libc::S_IFREG,
                    ..Default::default()
                },
                should_fail: true,
                assertions: Box::new(|base| -> Result<()> {
                    match fs::metadata(base.join("../not-shared")) {
                        Ok(_) => bail!("successful write outside shared directory"),
                        Err(_) => Ok(())
                    }
                }),
            },
        ];

        let uid = unistd::getuid().as_raw() as i32;
        let gid = unistd::getgid().as_raw() as i32;

        for mut tc in tests {
            println!("Running test case: {}", tc.name);
            // Since we're in a unit test, using root ownership causes issues with cleaning the temp dir.
            tc.request.uid = uid;
            tc.request.gid = gid;

            let res = do_copy_file(&tc.request, &base);
            if tc.should_fail != res.is_err() {
                panic!("{}: unexpected do_copy_file result: {:?}", tc.name, res)
            }
            (tc.assertions)(&base).context(tc.name).unwrap()
        }
    }

    #[test]
    fn test_map_ttrpc_err_preserves_not_found() {
        let not_found_err = ttrpc_error(ttrpc::Code::NOT_FOUND, "process not found");
        let anyhow_err: anyhow::Result<()> = Err(not_found_err.into());

        let result = anyhow_err.map_ttrpc_err(same);

        match &result.unwrap_err() {
            ttrpc::Error::RpcStatus(status) => {
                assert_eq!(status.code(), ttrpc::Code::NOT_FOUND);
            }
            other => panic!("expected RpcStatus, got: {:?}", other),
        }
    }

    #[test]
    fn test_map_ttrpc_err_wraps_non_ttrpc_as_internal() {
        let plain_err: anyhow::Result<()> = Err(anyhow!("something went wrong"));

        let result = plain_err.map_ttrpc_err(same);

        match &result.unwrap_err() {
            ttrpc::Error::RpcStatus(status) => {
                assert_eq!(status.code(), ttrpc::Code::INTERNAL);
            }
            other => panic!("expected RpcStatus, got: {:?}", other),
        }
    }

    // A cgroup Manager whose get_stats() blocks until released, used to prove that
    // stats_container does not hold the global sandbox lock across the (blocking) cgroup
    // read.
    struct BlockingStatsManager {
        started: Arc<std::sync::atomic::AtomicBool>,
        release: Arc<std::sync::atomic::AtomicBool>,
    }

    impl rustjail::cgroups::Manager for BlockingStatsManager {
        fn get_stats(&self) -> anyhow::Result<protocols::agent::CgroupStats> {
            self.started
                .store(true, std::sync::atomic::Ordering::SeqCst);
            while !self.release.load(std::sync::atomic::Ordering::SeqCst) {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            Ok(protocols::agent::CgroupStats::default())
        }

        fn name(&self) -> &str {
            "blocking-stats-test"
        }
    }

    // Regression test for the stats_container / kill starvation deadlock: while
    // stats_container is reading a container's cgroup stats (a blocking operation), the
    // global sandbox lock must remain free so other RPCs — crucially signal_process/kill —
    // can proceed. Previously the lock was held across get_stats(), so a stats read that
    // blocked on a torn-down cgroup during teardown wedged the whole agent and left the pod
    // stuck Terminating.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[serial]
    async fn test_stats_container_does_not_hold_sandbox_lock() {
        skip_if_not_root!();

        use std::sync::atomic::{AtomicBool, Ordering};

        let logger = slog::Logger::root(slog::Discard, o!());
        let (mut linux_container, _root) = create_linuxcontainer();
        linux_container.id = "1".to_string();

        let started = Arc::new(AtomicBool::new(false));
        let release = Arc::new(AtomicBool::new(false));

        // Always release the blocking get_stats() thread, even if the test panics before the
        // explicit release below. Tokio cannot cancel spawn_blocking tasks, so a still-looping
        // thread would otherwise hang the whole test binary at runtime shutdown.
        struct ReleaseOnDrop(Arc<AtomicBool>);
        impl Drop for ReleaseOnDrop {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }
        let _release_guard = ReleaseOnDrop(release.clone());

        linux_container.cgroup_manager = Arc::new(BlockingStatsManager {
            started: started.clone(),
            release: release.clone(),
        });

        let mut sandbox = Sandbox::new(&logger).unwrap();
        sandbox.add_container(linux_container);

        let agent_service = Arc::new(AgentService {
            sandbox: Arc::new(Mutex::new(sandbox)),
        });

        // Fire stats_container; get_stats() will block on the blocking thread pool.
        let svc = agent_service.clone();
        let handle = tokio::spawn(async move {
            svc.stats_container(
                &mk_ttrpc_context(),
                protocols::agent::StatsContainerRequest {
                    container_id: "1".to_string(),
                    ..Default::default()
                },
            )
            .await
        });

        // Wait until get_stats() is actually executing.
        for _ in 0..500 {
            if started.load(Ordering::SeqCst) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(started.load(Ordering::SeqCst), "get_stats never started");

        // The regression assertion: the sandbox lock must be acquirable while stats is
        // mid-read. With the old code (lock held across get_stats) this times out — exactly
        // the condition that starves the kill and wedges pod teardown.
        let lock_res = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            agent_service.sandbox.lock(),
        )
        .await;
        assert!(
            lock_res.is_ok(),
            "sandbox lock held during stats_container get_stats: signal_process/kill would be starved"
        );
        drop(lock_res);

        // Unblock the read and confirm the RPC completes successfully.
        release.store(true, Ordering::SeqCst);
        let res = handle.await.unwrap();
        assert!(res.is_ok(), "stats_container returned error: {:?}", res);
    }
}

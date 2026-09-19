// Copyright (c) 2019-2022 Alibaba Cloud
// Copyright (c) 2019-2022 Ant Group
//
// SPDX-License-Identifier: Apache-2.0
//

use std::fs;
use std::os::unix::io::RawFd;
use std::sync::Arc;

use anyhow::{Context, Result};
use common::message::{Action, Message};
use containerd_shim_protos::shim_async;
use kata_types::config::KATA_PATH;
use runtimes::RuntimeHandlerManager;
use tokio::sync::mpsc::{channel, Receiver};
use ttrpc::asynchronous::Server;

use crate::event::{new_event_publisher, ContainerdForwarder};
use crate::sandbox_service::SandboxService;
use crate::task_service::TaskService;
use containerd_shim_protos::sandbox_async;

/// message buffer size
const MESSAGE_BUFFER_SIZE: usize = 8;

pub struct ServiceManager {
    receiver: Receiver<Message>,
    server: Server,
    binary: String,
    address: String,
    namespace: String,
    event_publisher: ContainerdForwarder,
}

impl std::fmt::Debug for ServiceManager {
    // todo: some how to implement debug for handler
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServiceManager")
            .field("receiver", &self.receiver)
            .field("binary", &self.binary)
            .field("address", &self.address)
            .field("namespace", &self.namespace)
            .finish()
    }
}

impl ServiceManager {
    // TODO: who manages lifecycle for `task_server_fd`?
    pub async fn new(
        id: &str,
        containerd_binary: &str,
        address: &str,
        namespace: &str,
        task_server_fd: RawFd,
    ) -> Result<Self> {
        // Regist service logger for later use.
        logging::register_subsystem_logger("runtimes", "service");

        let (sender, receiver) = channel::<Message>(MESSAGE_BUFFER_SIZE);
        let rt_mgr = RuntimeHandlerManager::new(id, sender);
        let handler = Arc::new(rt_mgr);
        let sandbox_service: Arc<dyn sandbox_async::Sandbox + Send + Sync> =
            Arc::new(SandboxService::new(handler.clone()));
        let task_service: Arc<dyn shim_async::Task + Send + Sync> =
            Arc::new(TaskService::new(handler.clone()));
        // SAFETY: containerd passes a valid unix listener fd when starting the shim.
        let server = unsafe { Server::new().add_unix_listener(task_server_fd)? }
            .register_service(sandbox_async::create_sandbox(sandbox_service))
            .register_service(shim_async::create_task(task_service));
        let event_publisher = new_event_publisher(namespace)
            .await
            .context("new event publisher")?;

        Ok(Self {
            receiver,
            server,
            binary: containerd_binary.to_string(),
            address: address.to_string(),
            namespace: namespace.to_string(),
            event_publisher,
        })
    }

    pub async fn run(mut self) -> Result<()> {
        info!(sl!(), "begin to run service");
        self.server.start().await.context("start service")?;

        info!(sl!(), "wait server message");
        while let Some(r) = self.receiver.recv().await {
            info!(sl!(), "receive action {:?}", &r.action);
            match r.action {
                Action::Shutdown => {
                    self.server.stop_listen().await;
                    break;
                }
                Action::Event(event) => {
                    info!(sl!(), "get event {:?}", &event);
                    if let Err(err) = self.event_publisher.forward(event).await {
                        error!(sl!(), "failed to forward event: {:?}", err);
                    }
                }
            }
        }

        info!(sl!(), "end to run service");

        Ok(())
    }

    pub async fn cleanup(sid: &str) {
        let (sender, _receiver) = channel::<Message>(MESSAGE_BUFFER_SIZE);
        let handler = RuntimeHandlerManager::new(sid, sender);
        if let Err(e) = handler.cleanup().await {
            warn!(sl!(), "failed to clean up runtime state, {}", e);
        }

        let temp_dir = [KATA_PATH, sid].join("/");
        if fs::metadata(temp_dir.as_str()).is_ok() {
            // try to remove dir and skip the result
            if let Err(e) = fs::remove_dir_all(temp_dir) {
                warn!(sl!(), "failed to clean up sandbox tmp dir, {}", e);
            }
        }
    }
}

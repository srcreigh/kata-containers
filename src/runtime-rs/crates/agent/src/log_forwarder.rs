// Copyright (c) 2019-2022 Alibaba Cloud
// Copyright (c) 2019-2022 Ant Group
//
// SPDX-License-Identifier: Apache-2.0
//

use anyhow::Result;
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::sock;

pub(crate) struct LogForwarder {
    task_handler: Option<tokio::task::JoinHandle<()>>,
}

impl LogForwarder {
    pub(crate) fn new() -> Self {
        Self { task_handler: None }
    }

    pub(crate) fn stop(&mut self) {
        let task_handler = self.task_handler.take();
        if let Some(handler) = task_handler {
            handler.abort();
            info!(sl!(), "abort log forwarder thread");
        }
    }

    // start connect kata-agent log vsock and copy data to hypervisor's log stream
    pub(crate) async fn start(
        &mut self,
        address: &str,
        port: u32,
        config: sock::ConnectConfig,
    ) -> Result<()> {
        let logger = sl!().clone();
        let address = address.to_string();
        let task_handler = tokio::spawn(async move {
            let sock = match sock::new(&address, port) {
                Ok(sock) => sock,
                Err(err) => {
                    error!(
                        sl!(),
                        "failed to new sock for address {:?} port {} error {:?}",
                        address,
                        port,
                        err
                    );
                    return;
                }
            };
            info!(logger, "try to connect to agent-log");

            match sock.connect(&config).await {
                Ok(stream) => {
                    info!(logger, "connected to agent-log successfully");
                    let stream = BufReader::new(stream);
                    let mut lines = stream.lines();
                    while let Ok(Some(l)) = lines.next_line().await {
                        info!(sl!(), "guest agent: {}", l);
                    }
                }
                Err(err) => {
                    warn!(logger, "failed to connect agent-log, err: {:?}", err);
                }
            };
        });
        self.task_handler = Some(task_handler);
        Ok(())
    }
}

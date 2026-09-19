// Copyright (c) 2019-2022 Alibaba Cloud
// Copyright (c) 2019-2022 Ant Group
//
// SPDX-License-Identifier: Apache-2.0
//

use anyhow::Result;
use kata_sys_util::guest_io::{read_line, RateLimit};
use tokio::io::BufReader;

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
                    let mut stream = BufReader::new(stream);
                    let mut budget = RateLimit::new(1024, 1024 * 1024);
                    let result: std::io::Result<()> = async {
                        while let Some(line) = read_line(&mut stream, 16 * 1024).await? {
                            budget.check(line.len())?;
                            info!(sl!(), "guest agent: {}", line);
                        }
                        Ok(())
                    }
                    .await;
                    if let Err(error) = result {
                        warn!(sl!(), "guest log stream closed: {}", error);
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

// Copyright (c) 2019-2022 Alibaba Cloud
// Copyright (c) 2019-2022 Ant Group
//
// SPDX-License-Identifier: Apache-2.0
//

use std::{
    future::Future,
    io,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

use agent::Agent;
use anyhow::Result;
use common::types::ContainerProcess;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

struct ContainerIoInfo {
    pub agent: Arc<dyn Agent>,
    pub process: ContainerProcess,
}

pub struct ContainerIo {
    pub stdin: Box<dyn AsyncWrite + Send + Unpin>,
    pub stdout: Box<dyn AsyncRead + Send + Unpin>,
    pub stderr: Box<dyn AsyncRead + Send + Unpin>,
}

impl ContainerIo {
    pub fn new(agent: Arc<dyn Agent>, process: ContainerProcess) -> Self {
        let info = Arc::new(ContainerIoInfo { agent, process });

        Self {
            stdin: Box::new(ContainerIoWrite::new(info.clone())),
            stdout: Box::new(ContainerIoRead::new(info.clone(), true)),
            stderr: Box::new(ContainerIoRead::new(info, false)),
        }
    }
}

struct ContainerIoWrite {
    pub info: Arc<ContainerIoInfo>,
    write_future:
        Option<Pin<Box<dyn Future<Output = Result<agent::WriteStreamResponse>> + Send + 'static>>>,
    submitted: usize,
    shutdown_future:
        Option<Pin<Box<dyn Future<Output = Result<agent::WriteStreamResponse>> + Send + 'static>>>,
}

impl ContainerIoWrite {
    pub fn new(info: Arc<ContainerIoInfo>) -> Self {
        Self {
            info,
            write_future: Default::default(),
            submitted: 0,
            shutdown_future: Default::default(),
        }
    }

    fn poll_write_inner(&mut self, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        let mut write_future = self.write_future.take();
        if write_future.is_none() {
            let req = agent::WriteStreamRequest {
                process_id: self.info.process.clone().into(),
                data: buf.to_vec(),
            };
            self.submitted = buf.len();
            let info = self.info.clone();
            write_future = Some(Box::pin(async move { info.agent.write_stdin(req).await }));
        }

        let mut write_future = write_future.unwrap();
        match write_future.as_mut().poll(cx) {
            Poll::Ready(v) => match v {
                Ok(resp)
                    if resp.len as usize <= self.submitted && resp.len as usize <= buf.len() =>
                {
                    Poll::Ready(Ok(resp.len as usize))
                }
                Ok(_) => Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "guest write acknowledgement exceeds submitted bytes",
                ))),
                Err(err) => Poll::Ready(Err(std::io::Error::other(err))),
            },
            Poll::Pending => {
                self.write_future = Some(write_future);
                Poll::Pending
            }
        }
    }

    // Call rpc agent.write_stdin() with empty data to tell agent to close stdin of the process
    fn poll_shutdown_inner(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let mut shutdown_future = self.shutdown_future.take();
        if shutdown_future.is_none() {
            let req = agent::WriteStreamRequest {
                process_id: self.info.process.clone().into(),
                data: Vec::with_capacity(0),
            };
            let info = self.info.clone();
            shutdown_future = Some(Box::pin(async move { info.agent.write_stdin(req).await }));
        }

        let mut shutdown_future = shutdown_future.unwrap();
        match shutdown_future.as_mut().poll(cx) {
            Poll::Ready(v) => match v {
                Ok(resp) if resp.len == 0 => Poll::Ready(Ok(())),
                Ok(_) => Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "guest acknowledged bytes for empty stdin write",
                ))),
                Err(err) => Poll::Ready(Err(std::io::Error::other(err))),
            },
            Poll::Pending => {
                self.shutdown_future = Some(shutdown_future);
                Poll::Pending
            }
        }
    }
}

impl AsyncWrite for ContainerIoWrite {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.poll_write_inner(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.poll_shutdown_inner(cx)
    }
}

type ResultBuffer = Result<agent::ReadStreamResponse>;
struct ContainerIoRead {
    pub info: Arc<ContainerIoInfo>,
    is_stdout: bool,
    requested: usize,
    read_future: Option<Pin<Box<dyn Future<Output = ResultBuffer> + Send + 'static>>>,
}

impl ContainerIoRead {
    pub fn new(info: Arc<ContainerIoInfo>, is_stdout: bool) -> Self {
        Self {
            info,
            is_stdout,
            requested: 0,
            read_future: Default::default(),
        }
    }
    fn poll_read_inner(
        &mut self,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let mut read_future = self.read_future.take();
        if read_future.is_none() {
            let req = agent::ReadStreamRequest {
                process_id: self.info.process.clone().into(),
                len: buf.remaining() as u32,
            };
            self.requested = req.len as usize;
            let info = self.info.clone();
            let is_stdout = self.is_stdout;
            read_future = Some(Box::pin(async move {
                if is_stdout {
                    info.agent.read_stdout(req).await
                } else {
                    info.agent.read_stderr(req).await
                }
            }));
        }

        let mut read_future = read_future.unwrap();
        match read_future.as_mut().poll(cx) {
            Poll::Ready(v) => match v {
                Ok(resp) => {
                    if resp.data.len() > self.requested || resp.data.len() > buf.remaining() {
                        return Poll::Ready(Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "guest output exceeds requested buffer",
                        )));
                    }
                    buf.put_slice(&resp.data);
                    Poll::Ready(Ok(()))
                }
                Err(err) => Poll::Ready(Err(std::io::Error::other(err))),
            },
            Poll::Pending => {
                self.read_future = Some(read_future);
                Poll::Pending
            }
        }
    }
}

impl AsyncRead for ContainerIoRead {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        self.poll_read_inner(cx, buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn info() -> Arc<ContainerIoInfo> {
        Arc::new(ContainerIoInfo {
            agent: Arc::new(agent::kata::KataAgent::new(Default::default())),
            process: ContainerProcess::new("test-container", "").unwrap(),
        })
    }

    #[tokio::test]
    async fn guest_output_is_checked_against_request_and_current_buffer() {
        for (requested, capacity, response, valid) in [
            (4, 4, 4, true),
            (4, 4, 5, false),
            (8, 4, 5, false),
            (4, 8, 5, false),
            (4, 4, 0, true),
        ] {
            let mut reader = ContainerIoRead::new(info(), true);
            reader.requested = requested;
            reader.read_future = Some(Box::pin(async move {
                Ok(agent::ReadStreamResponse {
                    data: vec![b'x'; response],
                    ..Default::default()
                })
            }));
            let mut buffer = vec![0; capacity];
            let result = reader.read(&mut buffer).await;
            if valid {
                assert_eq!(result.unwrap(), response);
            } else {
                assert_eq!(result.unwrap_err().kind(), io::ErrorKind::InvalidData);
                assert!(buffer.iter().all(|b| *b == 0));
            }
        }
    }

    #[tokio::test]
    async fn guest_write_counts_and_shutdown_are_checked() {
        for length in [0, 2, 4, 5, u32::MAX] {
            let mut writer = ContainerIoWrite::new(info());
            writer.submitted = 4;
            writer.write_future = Some(Box::pin(async move {
                Ok(agent::WriteStreamResponse {
                    len: length,
                    ..Default::default()
                })
            }));
            let result = writer.write(b"data").await;
            if length <= 4 {
                assert_eq!(result.unwrap(), length as usize);
            } else {
                assert_eq!(result.unwrap_err().kind(), io::ErrorKind::InvalidData);
            }
        }
        let mut writer = ContainerIoWrite::new(info());
        writer.shutdown_future = Some(Box::pin(async {
            Ok(agent::WriteStreamResponse {
                len: 1,
                ..Default::default()
            })
        }));
        assert_eq!(
            writer.shutdown().await.unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }
}

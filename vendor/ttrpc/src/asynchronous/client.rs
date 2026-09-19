// Copyright 2022 Alibaba Cloud. All rights reserved.
// Copyright (c) 2020 Ant Financial
//
// SPDX-License-Identifier: Apache-2.0
//

use std::collections::HashMap;
use std::convert::TryInto;
#[cfg(unix)]
use std::os::unix::io::RawFd;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio::{self, sync::mpsc, task};

use crate::error::{get_rpc_status, Error, Result};
use crate::proto::{
    Code, Codec, GenMessage, Message, MessageHeader, Request, Response, FLAG_NO_DATA,
    FLAG_REMOTE_CLOSED, FLAG_REMOTE_OPEN, MESSAGE_TYPE_DATA, MESSAGE_TYPE_RESPONSE,
};
use crate::r#async::connection::*;
use crate::r#async::shutdown;
use crate::r#async::stream::{
    Kind, MessageReceiver, MessageSender, ResultReceiver, ResultSender, StreamInner,
};

use super::stream::SendingMessage;
use super::transport::Socket;

// Bound host-side outstanding unary work independently of guest cooperation.
const MAX_PENDING_REQUESTS: usize = 256;
struct Registration {
    id: u32,
    streams: Arc<Mutex<HashMap<u32, ResultSender>>>,
}
impl Drop for Registration {
    fn drop(&mut self) {
        if let Ok(mut streams) = self.streams.lock() {
            streams.remove(&self.id);
        }
    }
}

/// A ttrpc Client (async).
#[derive(Clone)]
pub struct Client {
    req_tx: MessageSender,
    next_stream_id: Arc<AtomicU32>,
    streams: Arc<Mutex<HashMap<u32, ResultSender>>>,
}

impl Client {
    pub async fn connect(sockaddr: &str) -> Result<Client> {
        let socket = Socket::connect(sockaddr)
            .await
            .map_err(err_to_others_err!(e, "Socket::connect error "))?;
        Ok(Self::new(socket))
    }

    #[cfg(unix)]
    /// # Safety
    /// The file descriptor must represent a unix socket.
    pub unsafe fn from_raw_unix_socket_fd(fd: RawFd) -> Client {
        let stream = unsafe { Socket::from_raw_unix_socket_fd(fd) }.unwrap();
        Self::new(stream)
    }

    /// Initialize a new [`Client`].
    pub fn new(stream: Socket) -> Client {
        let (req_tx, rx): (MessageSender, MessageReceiver) = mpsc::channel(100);

        let req_map = Arc::new(Mutex::new(HashMap::new()));
        let delegate = ClientBuilder {
            rx: Some(rx),
            streams: req_map.clone(),
        };

        let conn = Connection::new(stream, delegate);
        // Long-running receiver task
        tokio::spawn(async move { conn.run().await });

        Client {
            req_tx,
            next_stream_id: Arc::new(AtomicU32::new(1)),
            streams: req_map,
        }
    }

    /// Requsts a unary request and returns with response.
    pub async fn request(&self, req: Request) -> Result<Response> {
        let timeout_nano = req.timeout_nano;
        let stream_id = self.next_stream_id.fetch_add(2, Ordering::Relaxed);

        let msg: GenMessage = Message::new_request(stream_id, req)?
            .try_into()
            .map_err(|e: protobuf::Error| Error::Others(e.to_string()))?;

        let (tx, mut rx): (ResultSender, ResultReceiver) = mpsc::channel(1);

        {
            let mut streams = self
                .streams
                .lock()
                .map_err(|_| Error::Others("Failed to acquire lock on streams".to_string()))?;
            if streams.len() >= MAX_PENDING_REQUESTS || streams.contains_key(&stream_id) {
                return Err(Error::Others("too many pending RPC requests".to_string()));
            }
            streams.insert(stream_id, tx);
        }
        let _registration = Registration {
            id: stream_id,
            streams: self.streams.clone(),
        };

        let exchange = async {
            self.req_tx
                .send(SendingMessage::new(msg))
                .await
                .map_err(|_| Error::LocalClosed)?;
            rx.recv().await.ok_or(Error::RemoteClosed)
        };
        let result = if timeout_nano == 0 {
            exchange.await?
        } else {
            tokio::time::timeout(
                std::time::Duration::from_nanos(timeout_nano as u64),
                exchange,
            )
            .await
            .map_err(|e| Error::Others(format!("RPC timeout {e:?}")))??
        };

        let msg = result?;
        if msg.header.type_ != MESSAGE_TYPE_RESPONSE || msg.header.flags != 0 {
            return Err(Error::Others("invalid unary response frame".to_string()));
        }

        let res = Response::decode(msg.payload)
            .map_err(err_to_others_err!(e, "Unpack response error "))?;

        let status = res.status();
        if status.code() != Code::OK {
            return Err(Error::RpcStatus((*status).clone()));
        }

        Ok(res)
    }

    /// Creates a StreamInner instance.
    pub async fn new_stream(
        &self,
        req: Request,
        streaming_client: bool,
        streaming_server: bool,
    ) -> Result<StreamInner> {
        let stream_id = self.next_stream_id.fetch_add(2, Ordering::Relaxed);
        let is_req_payload_empty = req.payload.is_empty();

        let mut msg: GenMessage = Message::new_request(stream_id, req)?
            .try_into()
            .map_err(|e: protobuf::Error| Error::Others(e.to_string()))?;

        if streaming_client {
            if !is_req_payload_empty {
                return Err(get_rpc_status(
                    Code::INVALID_ARGUMENT,
                    "Creating a ClientStream and sending payload at the same time is not allowed",
                ));
            }
            msg.header.add_flags(FLAG_REMOTE_OPEN | FLAG_NO_DATA);
        } else {
            msg.header.add_flags(FLAG_REMOTE_CLOSED);
        }

        let (tx, rx): (ResultSender, ResultReceiver) = mpsc::channel(100);
        {
            let mut streams = self
                .streams
                .lock()
                .map_err(|_| Error::Others("Failed to acquire lock on streams".to_string()))?;
            if streams.len() >= MAX_PENDING_REQUESTS || streams.contains_key(&stream_id) {
                return Err(Error::Others("too many pending RPC requests".to_string()));
            }
            streams.insert(stream_id, tx);
        }

        self.req_tx
            .send(SendingMessage::new(msg))
            .await
            .map_err(|e| Error::Others(format!("Send packet to sender error {e:?}")))?;

        Ok(StreamInner::new(
            stream_id,
            self.req_tx.clone(),
            rx,
            streaming_client,
            streaming_server,
            Kind::Client,
            self.streams.clone(),
        ))
    }
}

#[derive(Debug)]
struct ClientBuilder {
    rx: Option<MessageReceiver>,
    streams: Arc<Mutex<HashMap<u32, ResultSender>>>,
}

impl Builder for ClientBuilder {
    type Reader = ClientReader;
    type Writer = ClientWriter;

    fn build(&mut self) -> (Self::Reader, Self::Writer) {
        let (notifier, waiter) = shutdown::new();
        (
            ClientReader {
                shutdown_waiter: waiter,
                streams: self.streams.clone(),
            },
            ClientWriter {
                rx: self.rx.take().unwrap(),
                shutdown_notifier: notifier,

                streams: self.streams.clone(),
            },
        )
    }
}

struct ClientWriter {
    rx: MessageReceiver,
    shutdown_notifier: shutdown::Notifier,

    streams: Arc<Mutex<HashMap<u32, ResultSender>>>,
}

#[async_trait]
impl WriterDelegate for ClientWriter {
    async fn recv(&mut self) -> Option<SendingMessage> {
        self.rx.recv().await
    }

    async fn disconnect(&self, msg: &GenMessage, e: Error) {
        // TODO:
        // At this point, a new request may have been received.
        let resp_tx = {
            let mut map = self.streams.lock().unwrap();
            map.remove(&msg.header.stream_id)
        };

        // TODO: if None
        if let Some(resp_tx) = resp_tx {
            let e = Error::Socket(format!("{e:?}"));
            resp_tx
                .send(Err(e))
                .await
                .unwrap_or_else(|_e| error!("The request has returned"));
        }
    }

    async fn exit(&self) {
        self.shutdown_notifier.shutdown();
    }
}

async fn get_resp_tx(
    req_map: Arc<Mutex<HashMap<u32, ResultSender>>>,
    header: &MessageHeader,
) -> Option<ResultSender> {
    let resp_tx = match header.type_ {
        MESSAGE_TYPE_RESPONSE => match req_map.lock().unwrap().remove(&header.stream_id) {
            Some(tx) => tx,
            None => {
                debug!("Receiver got unknown response packet {:?}", header);
                return None;
            }
        },
        MESSAGE_TYPE_DATA => {
            if (header.flags & FLAG_REMOTE_CLOSED) == FLAG_REMOTE_CLOSED {
                match req_map.lock().unwrap().remove(&header.stream_id) {
                    Some(tx) => tx,
                    None => {
                        debug!("Receiver got unknown data packet {:?}", header);
                        return None;
                    }
                }
            } else {
                match req_map.lock().unwrap().get(&header.stream_id) {
                    Some(tx) => tx.clone(),
                    None => {
                        debug!("Receiver got unknown data packet {:?}", header);
                        return None;
                    }
                }
            }
        }
        _ => {
            let resp_tx = match req_map.lock().unwrap().remove(&header.stream_id) {
                Some(tx) => tx,
                None => {
                    debug!("Receiver got unknown packet {:?}", header);
                    return None;
                }
            };
            let _ = resp_tx.try_send(Err(Error::Others("malformed response frame".into())));
            return None;
        }
    };

    Some(resp_tx)
}

struct ClientReader {
    streams: Arc<Mutex<HashMap<u32, ResultSender>>>,
    shutdown_waiter: shutdown::Waiter,
}

#[async_trait]
impl ReaderDelegate for ClientReader {
    async fn wait_shutdown(&self) {
        self.shutdown_waiter.wait_shutdown().await
    }

    async fn disconnect(&self, e: Error, sender: &mut task::JoinHandle<()>) {
        // Abort the request sender task to prevent incoming RPC requests
        // from being processed.
        sender.abort();
        let _ = sender.await;

        // Take all items out of `req_map`.
        let mut map = std::mem::take(&mut *self.streams.lock().unwrap());
        // Terminate undone RPC requests with the error.
        for (_stream_id, resp_tx) in map.drain() {
            if let Err(_e) = resp_tx.try_send(Err(e.clone())) {
                warn!("Failed to terminate pending RPC: the request has returned");
            }
        }
    }

    async fn exit(&self) {}

    async fn handle_err(&self, header: MessageHeader, e: Error) {
        if let Some(tx) = get_resp_tx(self.streams.clone(), &header).await {
            let _ = tx.try_send(Err(e));
        }
    }

    async fn handle_msg(&self, msg: GenMessage) {
        let id = msg.header.stream_id;
        if let Some(tx) = get_resp_tx(self.streams.clone(), &msg.header).await {
            if tx.try_send(Ok(msg)).is_err() {
                // A peer may not keep consuming bounded queues while their consumer stalls.
                self.streams.lock().unwrap().remove(&id);
            }
        }
    }
}

#[cfg(test)]
mod kata_security_tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

    fn pair() -> (Client, tokio::net::UnixStream) {
        let (host, guest) = tokio::net::UnixStream::pair().unwrap();
        (Client::new(host.into()), guest)
    }

    #[tokio::test]
    async fn timeouts_and_cancellation_release_registrations() {
        let (client, _guest) = pair();
        for _ in 0..8 {
            assert!(client
                .request(Request {
                    timeout_nano: 1_000_000,
                    ..Default::default()
                })
                .await
                .is_err());
            assert!(client.streams.lock().unwrap().is_empty());
        }
        let copy = client.clone();
        let task = tokio::spawn(async move { copy.request(Request::default()).await });
        while client.streams.lock().unwrap().is_empty() {
            tokio::task::yield_now().await;
        }
        task.abort();
        let _ = task.await;
        assert!(client.streams.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn pending_calls_are_bounded() {
        let (client, _guest) = pair();
        let mut tasks = Vec::new();
        for _ in 0..MAX_PENDING_REQUESTS {
            let copy = client.clone();
            tasks.push(tokio::spawn(async move {
                copy.request(Request::default()).await
            }));
        }
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while client.streams.lock().unwrap().len() < MAX_PENDING_REQUESTS {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(client.request(Request::default()).await.is_err());
        for task in tasks {
            task.abort();
            let _ = task.await;
        }
        assert!(client.streams.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn oversized_response_is_rejected_without_waiting_for_body() {
        let (client, mut guest) = pair();
        let peer = tokio::spawn(async move {
            let request = GenMessage::read_from(&mut guest).await.unwrap();
            let header = MessageHeader {
                length: (crate::proto::MESSAGE_LENGTH_MAX + 1) as u32,
                stream_id: request.header.stream_id,
                type_: MESSAGE_TYPE_RESPONSE,
                flags: 0,
            };
            guest.write_all(&Vec::<u8>::from(header)).await.unwrap();
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
        });
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            client.request(Request::default()),
        )
        .await
        .unwrap();
        assert!(result.is_err());
        assert!(client.streams.lock().unwrap().is_empty());
        peer.abort();
    }

    #[tokio::test]
    async fn unary_rejects_stream_data_then_accepts_an_ordinary_response() {
        let (client, mut guest) = pair();
        let peer = tokio::spawn(async move {
            for kind in [MESSAGE_TYPE_DATA, MESSAGE_TYPE_RESPONSE] {
                let request = GenMessage::read_from(&mut guest).await.unwrap();
                let response = GenMessage {
                    header: MessageHeader {
                        length: 0,
                        stream_id: request.header.stream_id,
                        type_: kind,
                        flags: 0,
                    },
                    payload: vec![],
                };
                response.write_to(&mut guest).await.unwrap();
            }
        });
        assert!(client.request(Request::default()).await.is_err());
        assert!(client.streams.lock().unwrap().is_empty());
        assert!(client.request(Request::default()).await.is_ok());
        peer.await.unwrap();
    }
}

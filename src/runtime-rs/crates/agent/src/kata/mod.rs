// Copyright (c) 2019-2022 Alibaba Cloud
// Copyright (c) 2019-2022 Ant Group
//
// SPDX-License-Identifier: Apache-2.0
//

mod agent;
mod response;
mod trans;

use std::sync::Arc;

use anyhow::{Context, Result};
use kata_types::config::Agent as AgentConfig;
use protobuf::Message;
use tokio::sync::RwLock;
use tracing_opentelemetry::OpenTelemetrySpanExt;
use ttrpc::asynchronous::Client;

use crate::{log_forwarder::LogForwarder, sock};

pub(crate) struct KataAgentInner {
    /// TTRPC client
    pub client: Option<Client>,

    /// Unix domain socket address
    pub socket_address: String,

    /// Agent config
    config: AgentConfig,

    /// Log forwarder
    log_forwarder: LogForwarder,
}

impl std::fmt::Debug for KataAgentInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KataAgentInner")
            .field("socket_address", &self.socket_address)
            .field("config", &self.config)
            .finish()
    }
}

#[derive(Debug)]
pub struct KataAgent {
    pub(crate) inner: Arc<RwLock<KataAgentInner>>,
}

impl KataAgent {
    pub fn new(config: AgentConfig) -> Self {
        KataAgent {
            inner: Arc::new(RwLock::new(KataAgentInner {
                client: None,
                socket_address: "".to_string(),
                config,
                log_forwarder: LogForwarder::new(),
            })),
        }
    }

    // No handwritten framing or status parser: ttrpc validates the envelope.
    #[tracing::instrument(skip_all, fields(rpc.service = service, rpc.method = method))]
    async fn request<M: Message>(
        &self,
        service: &str,
        method: &str,
        request: &M,
        timeout: Option<i64>,
    ) -> Result<Vec<u8>> {
        let (client, timeout_ms) = {
            let inner = self.inner.read().await;
            let configured = if service == "grpc.Health" {
                inner.config.health_check_request_timeout_ms
            } else {
                inner.config.request_timeout_ms
            };
            (
                inner.client.clone().context("agent is disconnected")?,
                timeout.unwrap_or(configured as i64),
            )
        };
        let mut carrier = std::collections::HashMap::new();
        opentelemetry::global::get_text_map_propagator(|p| {
            p.inject_context(&tracing::Span::current().context(), &mut carrier);
        });
        let response = client
            .request(ttrpc::Request {
                service: service.to_owned(),
                method: method.to_owned(),
                payload: request.write_to_bytes()?,
                metadata: carrier
                    .into_iter()
                    .map(|(key, value)| ttrpc::proto::KeyValue {
                        key,
                        value,
                        ..Default::default()
                    })
                    .collect(),
                timeout_nano: timeout_ms * 1_000_000,
                ..Default::default()
            })
            .await?;
        let limit = match method {
            "WaitProcess" | "WriteStdin" | "GetOOMEvent" => 4096,
            "ReadStdout" | "ReadStderr" | "GetDiagnosticData" | "GetVolumeStats" => 64 * 1024,
            "StatsContainer" | "GetMetrics" => 1024 * 1024,
            _ => 64 * 1024,
        };
        anyhow::ensure!(
            response.payload.len() <= limit,
            "guest {method} response exceeds byte limit"
        );
        Ok(response.payload)
    }

    pub(crate) async fn set_socket_address(&self, address: &str) -> Result<()> {
        let mut inner = self.inner.write().await;
        inner.socket_address = address.to_string();
        Ok(())
    }

    pub(crate) async fn connect_agent_server(&self) -> Result<()> {
        let mut inner = self.inner.write().await;

        let config = sock::ConnectConfig::new(
            inner.config.dial_timeout_ms as u64,
            inner.config.reconnect_timeout_ms as u64,
        );
        let sock =
            sock::new(&inner.socket_address, inner.config.server_port).context("new sock")?;
        info!(sl!(), "try to connect agent server through {:?}", sock);
        let stream = sock.connect(&config).await.context("connect")?;
        inner.client = Some(Client::new(stream.into()));
        Ok(())
    }

    pub(crate) async fn start_log_forwarder(&self) -> Result<()> {
        let mut inner = self.inner.write().await;
        let config = sock::ConnectConfig::new(
            inner.config.dial_timeout_ms as u64,
            inner.config.reconnect_timeout_ms as u64,
        );
        let address = inner.socket_address.clone();
        let port = inner.config.log_port;
        inner
            .log_forwarder
            .start(&address, port, config)
            .await
            .context("start log forwarder")?;
        Ok(())
    }

    pub(crate) async fn stop_log_forwarder(&self) {
        let mut inner = self.inner.write().await;
        inner.log_forwarder.stop();
    }

    pub(crate) async fn agent_sock(&self) -> Result<String> {
        let inner = self.inner.read().await;
        Ok(format!(
            "{}:{}",
            inner.socket_address.clone(),
            inner.config.server_port
        ))
    }

    /// Disconnect from the agent gRPC server and clean up related resources.
    pub(crate) async fn disconnect(&self) -> Result<()> {
        let mut inner = self.inner.write().await;
        inner.log_forwarder.stop();

        // If there is a valid client, drop it (closes the connection).
        inner.client.take();

        Ok(())
    }
}

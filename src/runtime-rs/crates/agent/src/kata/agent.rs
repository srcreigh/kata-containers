// Copyright (c) 2019-2022 Alibaba Cloud
// Copyright (c) 2019-2022 Ant Group
// SPDX-License-Identifier: Apache-2.0

use anyhow::{Context, Result};
use async_trait::async_trait;
use protobuf::Message;
use tracing::instrument;

use crate::{kata::KataAgent, Agent, AgentManager, HealthService};

#[async_trait]
impl AgentManager for KataAgent {
    #[instrument]
    async fn start(&self, address: &str) -> Result<()> {
        self.set_socket_address(address).await?;
        self.connect_agent_server().await.context("connect agent")?;
        self.start_log_forwarder()
            .await
            .context("connect log forwarder")
    }

    async fn stop(&self) {
        self.stop_log_forwarder().await;
    }
    async fn agent_sock(&self) -> Result<String> {
        self.agent_sock().await
    }
    async fn disconnect(&self) -> Result<()> {
        self.disconnect().await
    }
}

#[async_trait]
impl HealthService for KataAgent {
    async fn check(&self, req: crate::CheckRequest) -> Result<crate::Empty> {
        let request: protocols::health::CheckRequest = req.into();
        // The existing monitor uses RPC success as its liveness signal.
        // Do not decode the otherwise unused serving-status payload.
        self.request("grpc.Health", "Check", &request, None).await?;
        Ok(crate::Empty::new())
    }
}

// ttrpc owns framing, status validation and timeouts. Only the nine response
// bodies used by the runtime are decoded. Acknowledgements (including the
// unused network response bodies) require successful RPC status, not parsing.
macro_rules! impl_agent {
    (acks { $($ack:ident | $ack_wire:ident | $ack_req:ty | $ack_proto:ident),* $(,)? }
     replies { $($name:ident | $wire:ident | $req:ty | $proto:ident | $resp:ty | $timeout:expr),* $(,)? }) => {
        #[async_trait]
        impl Agent for KataAgent {
            $(
                #[instrument(skip(req))]
                async fn $ack(&self, req: $ack_req) -> Result<crate::Empty> {
                    let request: protocols::agent::$ack_proto = req.into();
                    self.request("grpc.AgentService", stringify!($ack_wire), &request, None).await?;
                    Ok(crate::Empty::new())
                }
            )*
            $(
                #[instrument(skip(req))]
                async fn $name(&self, req: $req) -> Result<$resp> {
                    let request: protocols::agent::$proto = req.into();
                    let payload = self.request("grpc.AgentService", stringify!($wire), &request, $timeout).await?;
                    Ok(<$resp>::parse_from_bytes(&payload)?)
                }
            )*
        }
    };
}

impl_agent! {
    acks {
        create_container | CreateContainer | crate::CreateContainerRequest | CreateContainerRequest,
        start_container | StartContainer | crate::ContainerID | StartContainerRequest,
        remove_container | RemoveContainer | crate::RemoveContainerRequest | RemoveContainerRequest,
        exec_process | ExecProcess | crate::ExecProcessRequest | ExecProcessRequest,
        signal_process | SignalProcess | crate::SignalProcessRequest | SignalProcessRequest,
        update_container | UpdateContainer | crate::UpdateContainerRequest | UpdateContainerRequest,
        pause_container | PauseContainer | crate::ContainerID | PauseContainerRequest,
        resume_container | ResumeContainer | crate::ContainerID | ResumeContainerRequest,
        tty_win_resize | TtyWinResize | crate::TtyWinResizeRequest | TtyWinResizeRequest,
        update_interface | UpdateInterface | crate::UpdateInterfaceRequest | UpdateInterfaceRequest,
        update_routes | UpdateRoutes | crate::UpdateRoutesRequest | UpdateRoutesRequest,
        add_arp_neighbors | AddARPNeighbors | crate::AddArpNeighborRequest | AddARPNeighborsRequest,
        create_sandbox | CreateSandbox | crate::CreateSandboxRequest | CreateSandboxRequest,
        copy_file | CopyFile | crate::CopyFileRequest | CopyFileRequest,
    }
    replies {
        wait_process | WaitProcess | crate::WaitProcessRequest | WaitProcessRequest | crate::WaitProcessResponse | Some(0),
        stats_container | StatsContainer | crate::ContainerID | StatsContainerRequest | crate::StatsContainerResponse | None,
        write_stdin | WriteStdin | crate::WriteStreamRequest | WriteStreamRequest | crate::WriteStreamResponse | Some(0),
        read_stdout | ReadStdout | crate::ReadStreamRequest | ReadStreamRequest | crate::ReadStreamResponse | Some(0),
        read_stderr | ReadStderr | crate::ReadStreamRequest | ReadStreamRequest | crate::ReadStreamResponse | Some(0),
        get_oom_event | GetOOMEvent | crate::Empty | GetOOMEventRequest | crate::OomEventResponse | Some(0),
        get_volume_stats | GetVolumeStats | crate::VolumeStatsRequest | VolumeStatsRequest | crate::VolumeStatsResponse | None,
        get_metrics | GetMetrics | crate::Empty | GetMetricsRequest | crate::MetricsResponse | None,
        get_diagnostic_data | GetDiagnosticData | crate::GetDiagnosticDataRequest | GetDiagnosticDataRequest | crate::GetDiagnosticDataResponse | None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::convert::TryInto;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    // Fake peer exercises the actual ttrpc client/envelope, not a mocked Agent.
    async fn peer(
        payload: Vec<u8>,
        code: ttrpc::Code,
    ) -> (KataAgent, tokio::task::JoinHandle<ttrpc::Request>) {
        let (host, mut guest) = tokio::net::UnixStream::pair().unwrap();
        let mut config = kata_types::config::Agent::default();
        config.request_timeout_ms = 1000;
        config.health_check_request_timeout_ms = 2000;
        let agent = KataAgent::new(config);
        agent.inner.write().await.client = Some(ttrpc::asynchronous::Client::new(host.into()));
        let task = tokio::spawn(async move {
            let mut header = [0u8; 10];
            guest.read_exact(&mut header).await.unwrap();
            let size = u32::from_be_bytes(header[..4].try_into().unwrap()) as usize;
            assert_eq!(header[8], 1);
            let mut body = vec![0; size];
            guest.read_exact(&mut body).await.unwrap();
            let request = ttrpc::Request::parse_from_bytes(&body).unwrap();
            let response = ttrpc::Response {
                status: protobuf::MessageField::some(ttrpc::get_status(code, "peer status")),
                payload,
                ..Default::default()
            }
            .write_to_bytes()
            .unwrap();
            header[..4].copy_from_slice(&(response.len() as u32).to_be_bytes());
            header[8] = 2;
            guest.write_all(&header).await.unwrap();
            guest.write_all(&response).await.unwrap();
            request
        });
        (agent, task)
    }

    #[tokio::test]
    async fn acknowledgements_ignore_bodies_but_preserve_rpc_errors() {
        for name in ["UpdateInterface", "UpdateRoutes", "CreateSandbox", "Check"] {
            let (agent, received) = peer(vec![0xff], ttrpc::Code::OK).await;
            // An invalid protobuf body must not be parsed for acknowledgement-only calls.
            match name {
                "UpdateInterface" => agent.update_interface(Default::default()).await.unwrap(),
                "UpdateRoutes" => agent.update_routes(Default::default()).await.unwrap(),
                "CreateSandbox" => agent.create_sandbox(Default::default()).await.unwrap(),
                _ => agent.check(Default::default()).await.unwrap(),
            };
            let request = received.await.unwrap();
            assert_eq!(request.method, name);
            assert_eq!(
                request.service,
                if name == "Check" {
                    "grpc.Health"
                } else {
                    "grpc.AgentService"
                }
            );
        }
        let (agent, received) = peer(vec![], ttrpc::Code::INVALID_ARGUMENT).await;
        let error = agent.update_routes(Default::default()).await.unwrap_err();
        assert!(
            matches!(error.downcast_ref::<ttrpc::Error>(), Some(ttrpc::Error::RpcStatus(s)) if s.code() == ttrpc::Code::INVALID_ARGUMENT)
        );
        received.await.unwrap();
    }

    #[tokio::test]
    async fn consumed_payloads_decode_and_preserve_timeouts() {
        let response = crate::WaitProcessResponse {
            status: 23,
            ..Default::default()
        };
        let (agent, received) = peer(response.write_to_bytes().unwrap(), ttrpc::Code::OK).await;
        assert_eq!(
            agent.wait_process(Default::default()).await.unwrap().status,
            23
        );
        let request = received.await.unwrap();
        assert_eq!(request.method, "WaitProcess");
        assert_eq!(request.timeout_nano, 0);
        let response = crate::ReadStreamResponse {
            data: b"stdout\0bytes".to_vec(),
            ..Default::default()
        };
        let (agent, received) = peer(response.write_to_bytes().unwrap(), ttrpc::Code::OK).await;
        assert_eq!(
            agent.read_stdout(Default::default()).await.unwrap().data,
            response.data
        );
        assert_eq!(received.await.unwrap().method, "ReadStdout");
        let (agent, received) = peer(vec![0xff], ttrpc::Code::OK).await;
        assert!(agent.stats_container(Default::default()).await.is_err());
        assert!(received.await.unwrap().timeout_nano > 0);
    }
}

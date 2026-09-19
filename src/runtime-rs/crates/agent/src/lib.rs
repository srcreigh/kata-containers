// Copyright (c) 2019-2022 Alibaba Cloud
// Copyright (c) 2019-2022 Ant Group
//
// SPDX-License-Identifier: Apache-2.0
//

#[macro_use]
extern crate slog;

logging::logger_with_subsystem!(sl, "agent");

pub mod kata;
mod log_forwarder;
mod sock;
pub mod types;
pub use protocols::agent::{
    BlkioStatsEntry, GetDiagnosticDataResponse, Metrics as MetricsResponse,
    OOMEvent as OomEventResponse, ReadStreamResponse, StatsContainerResponse, WaitProcessResponse,
    WriteStreamResponse,
};
pub use protocols::csi::VolumeStatsResponse;
pub use types::{
    ARPNeighbor, ARPNeighbors, AddArpNeighborRequest, CheckRequest, ContainerID,
    ContainerProcessID, CopyFileRequest, CreateContainerRequest, CreateSandboxRequest, Empty,
    ExecProcessRequest, FSGroup, FSGroupChangePolicy, GetDiagnosticDataRequest, IPAddress,
    IPFamily, Interface, ReadStreamRequest, RemoveContainerRequest, Route, Routes,
    SignalProcessRequest, Storage, TtyWinResizeRequest, UpdateContainerRequest,
    UpdateInterfaceRequest, UpdateRoutesRequest, VolumeStatsRequest, WaitProcessRequest,
    WriteStreamRequest,
};

use anyhow::Result;
use async_trait::async_trait;

pub const AGENT_KATA: &str = "kata";

#[async_trait]
pub trait AgentManager: Send + Sync {
    async fn start(&self, address: &str) -> Result<()>;
    async fn stop(&self);
    async fn disconnect(&self) -> Result<()>;

    async fn agent_sock(&self) -> Result<String>;
}

#[async_trait]
pub trait HealthService: Send + Sync {
    async fn check(&self, req: CheckRequest) -> Result<Empty>;
}

#[async_trait]
pub trait Agent: AgentManager + HealthService + Send + Sync {
    // sandbox
    async fn create_sandbox(&self, req: CreateSandboxRequest) -> Result<Empty>;

    // network
    async fn add_arp_neighbors(&self, req: AddArpNeighborRequest) -> Result<Empty>;
    async fn update_interface(&self, req: UpdateInterfaceRequest) -> Result<Empty>;
    async fn update_routes(&self, req: UpdateRoutesRequest) -> Result<Empty>;

    // container
    async fn create_container(&self, req: CreateContainerRequest) -> Result<Empty>;
    async fn pause_container(&self, req: ContainerID) -> Result<Empty>;
    async fn remove_container(&self, req: RemoveContainerRequest) -> Result<Empty>;
    async fn resume_container(&self, req: ContainerID) -> Result<Empty>;
    async fn start_container(&self, req: ContainerID) -> Result<Empty>;
    async fn stats_container(&self, req: ContainerID) -> Result<StatsContainerResponse>;
    async fn update_container(&self, req: UpdateContainerRequest) -> Result<Empty>;

    // process
    async fn exec_process(&self, req: ExecProcessRequest) -> Result<Empty>;
    async fn signal_process(&self, req: SignalProcessRequest) -> Result<Empty>;
    async fn wait_process(&self, req: WaitProcessRequest) -> Result<WaitProcessResponse>;

    // io and tty
    async fn read_stderr(&self, req: ReadStreamRequest) -> Result<ReadStreamResponse>;
    async fn read_stdout(&self, req: ReadStreamRequest) -> Result<ReadStreamResponse>;
    async fn tty_win_resize(&self, req: TtyWinResizeRequest) -> Result<Empty>;
    async fn write_stdin(&self, req: WriteStreamRequest) -> Result<WriteStreamResponse>;

    // utils
    async fn copy_file(&self, req: CopyFileRequest) -> Result<Empty>;
    async fn get_metrics(&self, req: Empty) -> Result<MetricsResponse>;
    async fn get_oom_event(&self, req: Empty) -> Result<OomEventResponse>;
    async fn get_volume_stats(&self, req: VolumeStatsRequest) -> Result<VolumeStatsResponse>;

    // diagnostics
    async fn get_diagnostic_data(
        &self,
        req: GetDiagnosticDataRequest,
    ) -> Result<GetDiagnosticDataResponse>;
}

// Copyright (c) 2019-2022 Alibaba Cloud
// Copyright (c) 2019-2022 Ant Group
//
// SPDX-License-Identifier: Apache-2.0
//

use serde::Deserialize;

use oci_spec::runtime as oci;

pub const DEFAULT_REMOVE_CONTAINER_REQUEST_TIMEOUT: u32 = 10;

#[derive(Debug, PartialEq, Clone, Default)]
pub struct Empty {}
impl Empty {
    pub fn new() -> Self {
        Self::default()
    }
}

#[derive(Default, Debug, Clone, PartialEq)]
pub enum FSGroupChangePolicy {
    #[default]
    Always = 0,
    OnRootMismatch = 1,
}

#[derive(Debug, PartialEq, Clone, Default)]
pub struct FSGroup {
    pub group_id: u32,
    pub group_change_policy: FSGroupChangePolicy,
}

#[derive(PartialEq, Clone, Default)]
pub struct StringUser {
    pub uid: String,
    pub gid: String,
    pub additional_gids: Vec<String>,
}

#[derive(Debug, PartialEq, Clone, Default)]
pub struct Storage {
    pub driver: String,
    pub driver_options: Vec<String>,
    pub source: String,
    pub fs_type: String,
    pub fs_group: Option<FSGroup>,
    pub options: Vec<String>,
    pub mount_point: String,
    pub shared: bool,
}

#[derive(Deserialize, Default, Clone, PartialEq, Eq, Debug, Hash)]
pub enum IPFamily {
    #[default]
    V4 = 0,
    V6 = 1,
}

#[derive(Deserialize, Debug, PartialEq, Clone, Default)]
pub struct IPAddress {
    pub family: IPFamily,
    pub address: String,
    pub mask: String,
}

#[derive(Deserialize, Debug, PartialEq, Clone, Default)]
pub struct Interface {
    pub device: String,
    pub name: String,
    pub ip_addresses: Vec<IPAddress>,
    pub mtu: u64,
    pub hw_addr: String,
    #[serde(default)]
    pub device_path: String,
    #[serde(default)]
    pub field_type: String,
    #[serde(default)]
    pub raw_flags: u32,
}

#[derive(Deserialize, Debug, PartialEq, Clone, Default)]
pub struct Route {
    pub dest: String,
    pub gateway: String,
    pub device: String,
    pub source: String,
    pub scope: u32,
    pub family: IPFamily,
    pub flags: u32,
    pub mtu: u32,
}

#[derive(Deserialize, Debug, PartialEq, Clone, Default)]
pub struct Routes {
    pub routes: Vec<Route>,
}

#[derive(PartialEq, Clone, Default)]
pub struct CreateContainerRequest {
    pub process_id: ContainerProcessID,
    pub string_user: Option<StringUser>,
    pub storages: Vec<Storage>,
    pub oci: Option<oci::Spec>,
    pub sandbox_pidns: bool,
    pub rootfs_mounts: Vec<oci::Mount>,
    pub stdin_port: Option<u32>,
    pub stdout_port: Option<u32>,
    pub stderr_port: Option<u32>,
}

#[derive(PartialEq, Clone, Default)]
pub struct ContainerID {
    pub container_id: String,
}

impl ContainerID {
    pub fn new(id: &str) -> Self {
        Self {
            container_id: id.to_string(),
        }
    }
}

#[derive(PartialEq, Clone, Default)]
pub struct ContainerProcessID {
    pub container_id: ContainerID,
    pub exec_id: String,
}

impl ContainerProcessID {
    pub fn new(container_id: &str, exec_id: &str) -> Self {
        Self {
            container_id: ContainerID::new(container_id),
            exec_id: exec_id.to_string(),
        }
    }

    pub fn container_id(&self) -> String {
        self.container_id.container_id.clone()
    }

    pub fn exec_id(&self) -> String {
        self.exec_id.clone()
    }
}

#[derive(PartialEq, Clone, Debug)]
pub struct RemoveContainerRequest {
    pub container_id: String,
    pub timeout: u32,
}

impl RemoveContainerRequest {
    pub fn new(id: &str, timeout: u32) -> Self {
        Self {
            container_id: id.to_string(),
            timeout,
        }
    }
}

impl std::default::Default for RemoveContainerRequest {
    fn default() -> Self {
        Self {
            container_id: "".to_string(),
            timeout: DEFAULT_REMOVE_CONTAINER_REQUEST_TIMEOUT,
        }
    }
}

#[derive(PartialEq, Clone, Default)]
pub struct SignalProcessRequest {
    pub process_id: ContainerProcessID,
    pub signal: u32,
}

#[derive(PartialEq, Clone, Default)]
pub struct WaitProcessRequest {
    pub process_id: ContainerProcessID,
}

#[derive(PartialEq, Clone, Default)]
pub struct UpdateContainerRequest {
    pub container_id: String,
    pub resources: Option<oci::LinuxResources>,
    pub mounts: Vec<oci::Mount>,
}

#[derive(PartialEq, Clone, Default)]
pub struct WriteStreamRequest {
    pub process_id: ContainerProcessID,
    pub data: Vec<u8>,
}

#[derive(PartialEq, Clone, Default)]
pub struct ExecProcessRequest {
    pub process_id: ContainerProcessID,
    pub string_user: Option<StringUser>,
    pub process: Option<oci::Process>,
    pub stdin_port: Option<u32>,
    pub stdout_port: Option<u32>,
    pub stderr_port: Option<u32>,
}

#[derive(PartialEq, Clone, Default)]
pub struct ReadStreamRequest {
    pub process_id: ContainerProcessID,
    pub len: u32,
}

#[derive(PartialEq, Clone, Default)]
pub struct TtyWinResizeRequest {
    pub process_id: ContainerProcessID,
    pub row: u32,
    pub column: u32,
}

#[derive(Debug, PartialEq, Clone, Default)]
pub struct UpdateInterfaceRequest {
    pub interface: Option<Interface>,
}

#[derive(PartialEq, Clone, Default, Debug)]
pub struct UpdateRoutesRequest {
    pub route: Option<Routes>,
}

#[derive(Deserialize, PartialEq, Clone, Default, Debug)]
pub struct ARPNeighbor {
    pub to_ip_address: Option<IPAddress>,
    pub device: String,
    pub ll_addr: String,
    pub state: i32,
    pub flags: i32,
}

#[derive(PartialEq, Clone, Default, Debug)]
pub struct ARPNeighbors {
    pub neighbors: Vec<ARPNeighbor>,
}

#[derive(PartialEq, Clone, Default, Debug)]
pub struct AddArpNeighborRequest {
    pub neighbors: Option<ARPNeighbors>,
}

#[derive(PartialEq, Clone, Default)]
pub struct CreateSandboxRequest {
    pub hostname: String,
    pub dns: Vec<String>,
    pub storages: Vec<Storage>,
    pub sandbox_pidns: bool,
    pub sandbox_id: String,
}

#[derive(PartialEq, Clone, Default)]
pub struct CopyFileRequest {
    pub path: String,
    pub file_size: i64,
    pub file_mode: u32,
    pub dir_mode: u32,
    pub uid: i32,
    pub gid: i32,
    pub offset: i64,
    pub data: ::std::vec::Vec<u8>,
}

#[derive(PartialEq, Clone, Default, Debug)]
pub struct CheckRequest {
    pub service: String,
}

impl CheckRequest {
    pub fn new(service: &str) -> Self {
        Self {
            service: service.to_string(),
        }
    }
}

#[derive(PartialEq, Clone, Default, Debug)]
pub struct VolumeStatsRequest {
    pub volume_guest_path: String,
}

#[derive(PartialEq, Clone, Default, Debug)]
pub struct GetDiagnosticDataRequest {
    pub log_type: String,
    pub container_id: String,
}

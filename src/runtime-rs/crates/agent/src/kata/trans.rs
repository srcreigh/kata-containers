// Copyright (c) 2019-2022 Alibaba Cloud
// Copyright (c) 2019-2022 Ant Group
//
// SPDX-License-Identifier: Apache-2.0
//

use std::convert::Into;

use crate::types::Device;
use crate::{
    ARPNeighbor, ARPNeighbors, AddArpNeighborRequest, CheckRequest, ContainerID, CopyFileRequest,
    CreateContainerRequest, CreateSandboxRequest, Empty, ExecProcessRequest, FSGroup,
    FSGroupChangePolicy, GetDiagnosticDataRequest, IPAddress, IPFamily, Interface,
    ReadStreamRequest, RemoveContainerRequest, Route, Routes, SignalProcessRequest, Storage,
    StringUser, TtyWinResizeRequest, UpdateContainerRequest, UpdateInterfaceRequest,
    UpdateRoutesRequest, VolumeStatsRequest, WaitProcessRequest, WriteStreamRequest,
};
use protocols::{agent, health, types};

fn trans_vec<F: Sized, T: From<F>>(from: Vec<F>) -> Vec<T> {
    from.into_iter().map(|f| f.into()).collect()
}

fn from_option<F: Sized, T: From<F>>(from: Option<F>) -> protobuf::MessageField<T> {
    match from {
        Some(f) => protobuf::MessageField::from_option(Some(T::from(f))),
        None => protobuf::MessageField::none(),
    }
}

impl From<FSGroup> for agent::FSGroup {
    fn from(from: FSGroup) -> Self {
        let policy = match from.group_change_policy {
            FSGroupChangePolicy::Always => types::FSGroupChangePolicy::Always,
            FSGroupChangePolicy::OnRootMismatch => types::FSGroupChangePolicy::OnRootMismatch,
        };

        Self {
            group_id: from.group_id,
            group_change_policy: policy.into(),
            ..Default::default()
        }
    }
}

impl From<StringUser> for agent::StringUser {
    fn from(from: StringUser) -> Self {
        Self {
            uid: from.uid,
            gid: from.gid,
            additionalGids: from.additional_gids,
            ..Default::default()
        }
    }
}

impl From<Storage> for agent::Storage {
    fn from(from: Storage) -> Self {
        Self {
            driver: from.driver,
            driver_options: trans_vec(from.driver_options),
            source: from.source,
            fstype: from.fs_type,
            fs_group: from_option(from.fs_group),
            options: trans_vec(from.options),
            mount_point: from.mount_point,
            shared: from.shared,
            ..Default::default()
        }
    }
}

impl From<IPFamily> for types::IPFamily {
    fn from(from: IPFamily) -> Self {
        if from == IPFamily::V4 {
            types::IPFamily::v4
        } else {
            types::IPFamily::v6
        }
    }
}

impl From<IPAddress> for types::IPAddress {
    fn from(from: IPAddress) -> Self {
        Self {
            family: protobuf::EnumOrUnknown::new(from.family.into()),
            address: from.address,
            mask: from.mask,
            ..Default::default()
        }
    }
}

impl From<Interface> for types::Interface {
    fn from(from: Interface) -> Self {
        Self {
            device: from.device,
            name: from.name,
            IPAddresses: trans_vec(from.ip_addresses),
            mtu: from.mtu,
            hwAddr: from.hw_addr,
            devicePath: from.device_path,
            type_: from.field_type,
            raw_flags: from.raw_flags,
            ..Default::default()
        }
    }
}

impl From<Route> for types::Route {
    fn from(from: Route) -> Self {
        Self {
            dest: from.dest,
            gateway: from.gateway,
            device: from.device,
            source: from.source,
            scope: from.scope,
            family: protobuf::EnumOrUnknown::new(from.family.into()),
            flags: from.flags,
            mtu: from.mtu,
            ..Default::default()
        }
    }
}

impl From<Routes> for agent::Routes {
    fn from(from: Routes) -> Self {
        Self {
            Routes: trans_vec(from.routes),
            ..Default::default()
        }
    }
}

impl From<CreateContainerRequest> for agent::CreateContainerRequest {
    fn from(from: CreateContainerRequest) -> Self {
        Self {
            container_id: from.process_id.container_id(),
            exec_id: from.process_id.exec_id(),
            devices: trans_vec(from.devices),
            string_user: from_option(from.string_user),
            storages: trans_vec(from.storages),
            OCI: from_option(from.oci),
            sandbox_pidns: from.sandbox_pidns,
            stdin_port: from.stdin_port.unwrap_or_default(),
            stdout_port: from.stdout_port.unwrap_or_default(),
            stderr_port: from.stderr_port.unwrap_or_default(),
            ..Default::default()
        }
    }
}

impl From<RemoveContainerRequest> for agent::RemoveContainerRequest {
    fn from(from: RemoveContainerRequest) -> Self {
        Self {
            container_id: from.container_id,
            timeout: from.timeout,
            ..Default::default()
        }
    }
}

impl From<ContainerID> for agent::StartContainerRequest {
    fn from(from: ContainerID) -> Self {
        Self {
            container_id: from.container_id,
            ..Default::default()
        }
    }
}

impl From<ContainerID> for agent::StatsContainerRequest {
    fn from(from: ContainerID) -> Self {
        Self {
            container_id: from.container_id,
            ..Default::default()
        }
    }
}

impl From<ContainerID> for agent::PauseContainerRequest {
    fn from(from: ContainerID) -> Self {
        Self {
            container_id: from.container_id,
            ..Default::default()
        }
    }
}

impl From<ContainerID> for agent::ResumeContainerRequest {
    fn from(from: ContainerID) -> Self {
        Self {
            container_id: from.container_id,
            ..Default::default()
        }
    }
}

impl From<SignalProcessRequest> for agent::SignalProcessRequest {
    fn from(from: SignalProcessRequest) -> Self {
        Self {
            container_id: from.process_id.container_id(),
            exec_id: from.process_id.exec_id(),
            signal: from.signal,
            ..Default::default()
        }
    }
}

impl From<WaitProcessRequest> for agent::WaitProcessRequest {
    fn from(from: WaitProcessRequest) -> Self {
        Self {
            container_id: from.process_id.container_id(),
            exec_id: from.process_id.exec_id(),
            ..Default::default()
        }
    }
}

impl From<UpdateContainerRequest> for agent::UpdateContainerRequest {
    fn from(from: UpdateContainerRequest) -> Self {
        Self {
            container_id: from.container_id,
            resources: from_option(from.resources),
            ..Default::default()
        }
    }
}

impl From<WriteStreamRequest> for agent::WriteStreamRequest {
    fn from(from: WriteStreamRequest) -> Self {
        Self {
            container_id: from.process_id.container_id(),
            exec_id: from.process_id.exec_id(),
            data: from.data,
            ..Default::default()
        }
    }
}

impl From<ExecProcessRequest> for agent::ExecProcessRequest {
    fn from(from: ExecProcessRequest) -> Self {
        Self {
            container_id: from.process_id.container_id(),
            exec_id: from.process_id.exec_id(),
            string_user: from_option(from.string_user),
            process: from_option(from.process),
            stdin_port: from.stdin_port.unwrap_or_default(),
            stdout_port: from.stdout_port.unwrap_or_default(),
            stderr_port: from.stderr_port.unwrap_or_default(),
            ..Default::default()
        }
    }
}

impl From<ReadStreamRequest> for agent::ReadStreamRequest {
    fn from(from: ReadStreamRequest) -> Self {
        Self {
            container_id: from.process_id.container_id(),
            exec_id: from.process_id.exec_id(),
            len: from.len,
            ..Default::default()
        }
    }
}

impl From<TtyWinResizeRequest> for agent::TtyWinResizeRequest {
    fn from(from: TtyWinResizeRequest) -> Self {
        Self {
            container_id: from.process_id.container_id(),
            exec_id: from.process_id.exec_id(),
            row: from.row,
            column: from.column,
            ..Default::default()
        }
    }
}

impl From<UpdateInterfaceRequest> for agent::UpdateInterfaceRequest {
    fn from(from: UpdateInterfaceRequest) -> Self {
        Self {
            interface: from_option(from.interface),
            ..Default::default()
        }
    }
}

impl From<UpdateRoutesRequest> for agent::UpdateRoutesRequest {
    fn from(from: UpdateRoutesRequest) -> Self {
        Self {
            routes: from_option(from.route),
            ..Default::default()
        }
    }
}

impl From<ARPNeighbor> for types::ARPNeighbor {
    fn from(from: ARPNeighbor) -> Self {
        Self {
            toIPAddress: from_option(from.to_ip_address),
            device: from.device,
            lladdr: from.ll_addr,
            state: from.state,
            flags: from.flags,
            ..Default::default()
        }
    }
}

impl From<ARPNeighbors> for agent::ARPNeighbors {
    fn from(from: ARPNeighbors) -> Self {
        Self {
            ARPNeighbors: trans_vec(from.neighbors),
            ..Default::default()
        }
    }
}

impl From<AddArpNeighborRequest> for agent::AddARPNeighborsRequest {
    fn from(from: AddArpNeighborRequest) -> Self {
        Self {
            neighbors: from_option(from.neighbors),
            ..Default::default()
        }
    }
}

impl From<CreateSandboxRequest> for agent::CreateSandboxRequest {
    fn from(from: CreateSandboxRequest) -> Self {
        Self {
            hostname: from.hostname,
            dns: trans_vec(from.dns),
            storages: trans_vec(from.storages),
            sandbox_pidns: from.sandbox_pidns,
            sandbox_id: from.sandbox_id,
            ..Default::default()
        }
    }
}

impl From<GetDiagnosticDataRequest> for agent::GetDiagnosticDataRequest {
    fn from(from: GetDiagnosticDataRequest) -> Self {
        Self {
            log_type: from.log_type,
            container_id: from.container_id,
            ..Default::default()
        }
    }
}

impl From<CopyFileRequest> for agent::CopyFileRequest {
    fn from(from: CopyFileRequest) -> Self {
        Self {
            path: from.path,
            file_size: from.file_size,
            file_mode: from.file_mode,
            dir_mode: from.dir_mode,
            uid: from.uid,
            gid: from.gid,
            offset: from.offset,
            data: from.data,
            ..Default::default()
        }
    }
}

impl From<Empty> for agent::GetMetricsRequest {
    fn from(_: Empty) -> Self {
        Self {
            ..Default::default()
        }
    }
}

impl From<Empty> for agent::GetOOMEventRequest {
    fn from(_: Empty) -> Self {
        Self {
            ..Default::default()
        }
    }
}

impl From<CheckRequest> for health::CheckRequest {
    fn from(from: CheckRequest) -> Self {
        Self {
            service: from.service,
            ..Default::default()
        }
    }
}

impl From<VolumeStatsRequest> for agent::VolumeStatsRequest {
    fn from(from: VolumeStatsRequest) -> Self {
        Self {
            volume_guest_path: from.volume_guest_path,
            ..Default::default()
        }
    }
}

impl From<Device> for agent::Device {
    fn from(from: Device) -> Self {
        Self {
            id: from.id,
            type_: from.field_type,
            vm_path: from.vm_path,
            container_path: from.container_path,
            options: trans_vec(from.options),
            ..Default::default()
        }
    }
}

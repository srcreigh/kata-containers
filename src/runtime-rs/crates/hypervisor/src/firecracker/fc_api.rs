//Copyright (c) 2019-2022 Alibaba Cloud
//Copyright (c) 2023 Nubificus Ltd
//
//SPDX-License-Identifier: Apache-2.0

use super::mac::MacAddr;
use crate::{
    firecracker::{
        inner_hypervisor::{FC_AGENT_SOCKET_NAME, ROOT},
        sl, FcInner,
    },
    kernel_param::KernelParams,
    NetworkConfig, Param,
};
use anyhow::{anyhow, Context, Result};
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::{body::Incoming, Method, Request, Response};
use hyperlocal::Uri;
use kata_sys_util::mount;
use kata_types::config::hypervisor::RateLimiterConfig;
use nix::mount::MsFlags;
use serde_json::json;
use tokio::{fs, fs::File};

const REQUEST_RETRY: u32 = 500;
const MAX_ERROR_BODY: usize = 64 * 1024;
const FC_KERNEL: &str = "vmlinux";
const FC_ROOT_FS: &str = "rootfs";
const DRIVE_PREFIX: &str = "drive";
const DISK_POOL_SIZE: u32 = 6;

impl FcInner {
    pub(crate) fn get_resource(&self, src: &str, dst: &str) -> Result<String> {
        self.jail_resource(src, dst)
    }

    fn jail_resource(&self, src: &str, dst: &str) -> Result<String> {
        if src.is_empty() || dst.is_empty() {
            return Err(anyhow!("invalid param src {} dst {}", src, dst));
        }

        let jailed_location = [self.vm_path.as_str(), ROOT, dst].join("/");
        mount::bind_mount_unchecked(src, jailed_location.as_str(), false, MsFlags::MS_SLAVE)
            .context("bind_mount ERROR")?;

        let mut abs_path = String::from("/");
        abs_path.push_str(dst);
        Ok(abs_path)
    }

    // Remounting jailer root to ensure it has exec permissions, since firecracker binary will
    // execute from there
    pub(crate) async fn remount_jailer_with_exec(&self) -> Result<()> {
        let localpath = [self.vm_path.clone(), ROOT.to_string()].join("/");
        let _ = fs::create_dir_all(&localpath)
            .await
            .context(format!("failed to create directory {:?}", &localpath));
        mount::bind_mount_unchecked(&localpath, &localpath, false, MsFlags::MS_SHARED)
            .context("bind mount jailer root")?;

        mount::bind_remount(&localpath, false).context("rebind mount jailer root")?;
        Ok(())
    }

    pub(crate) async fn prepare_hvsock(&mut self) -> Result<()> {
        let body_vsock: String = json!({
            "guest_cid": 3,
            "uds_path": FC_AGENT_SOCKET_NAME,
            "vsock_id": ROOT,
        })
        .to_string();

        self.request_with_retry(Method::PUT, "/vsock", body_vsock)
            .await?;
        Ok(())
    }

    pub(crate) async fn prepare_vmm_resources(&mut self) -> Result<()> {
        let mut kernel_params = KernelParams::new(self.config.debug_info.enable_debug);
        kernel_params.push(Param::new("pci", "off"));
        kernel_params.push(Param::new("iommu", "off"));
        let mut rootfs_params = KernelParams::new_rootfs_kernel_params(
            &self.config.boot_info.kernel_verity_params,
            &self.config.blockdev_info.block_device_driver,
            &self.config.boot_info.rootfs_type,
        )?;
        kernel_params.append(&mut rootfs_params);
        kernel_params.append(&mut KernelParams::from_string(
            &self.config.boot_info.kernel_params,
        ));
        let mut parameters = String::new().to_owned();

        if let Ok(param) = &kernel_params.to_string() {
            parameters.push_str(&param.to_string());
        }

        let kernel = self
            .get_resource(&self.config.boot_info.kernel, FC_KERNEL)
            .context("get resource KERNEL")?;
        let rootfs = self
            .get_resource(&self.config.boot_info.image, FC_ROOT_FS)
            .context("get resource ROOTFS")?;

        let body_config: String = json!({
            "mem_size_mib": self.config.memory_info.default_memory,
            "vcpu_count": self.config.cpu_info.default_vcpus.ceil() as u8,
        })
        .to_string();
        let body_kernel: String = json!({
            "kernel_image_path": kernel,
            "boot_args": parameters,
        })
        .to_string();

        let body_rootfs: String = json!({
            "drive_id": "rootfs",
            "path_on_host": rootfs,
            "is_root_device": false,
            "is_read_only": true
        })
        .to_string();

        info!(sl(), "Before first request");
        self.request_with_retry(Method::PUT, "/boot-source", body_kernel)
            .await?;
        self.request_with_retry(Method::PUT, "/machine-config", body_config)
            .await?;
        self.request_with_retry(Method::PUT, "/drives/rootfs", body_rootfs)
            .await?;

        let abs_path = [&self.vm_path, ROOT].join("/");

        let _ = fs::create_dir_all(&abs_path)
            .await
            .context(format!("failed to create directory {:?}", &abs_path));

        // We create some placeholder drives to be used for patching block devices while the vmm is
        // running, as firecracker does not support device hotplug.
        for i in 1..DISK_POOL_SIZE {
            let full_path_name = format!("{abs_path}/drive{i}");

            let _ = File::create(&full_path_name)
                .await
                .context(format!("failed to create file {:?}", &full_path_name));

            let body: String = json!({
                "drive_id": format!("drive{}",i),
                "path_on_host": format!("/drive{}", i),
                "is_root_device": false,
                "is_read_only": false
            })
            .to_string();

            self.request_with_retry(Method::PUT, &format!("/drives/drive{i}"), body)
                .await?;
        }

        Ok(())
    }
    pub(crate) async fn patch_container_rootfs(
        &mut self,
        drive_id: &str,
        drive_path: &str,
    ) -> Result<()> {
        let new_drive_id = &[DRIVE_PREFIX, drive_id].concat();
        let new_drive_path = self
            .get_resource(drive_path, new_drive_id)
            .context("get resource CONTAINER ROOTFS")?;

        let block_rate_limit = RateLimiterConfig::new(
            self.config.blockdev_info.disk_rate_limiter_bw_max_rate,
            self.config.blockdev_info.disk_rate_limiter_ops_max_rate,
            self.config
                .blockdev_info
                .disk_rate_limiter_bw_one_time_burst,
            self.config
                .blockdev_info
                .disk_rate_limiter_ops_one_time_burst,
        );

        let body: String = json!({
            "drive_id": format!("drive{drive_id}"),
            "path_on_host": new_drive_path,
            "rate_limiter": block_rate_limit,
        })
        .to_string();
        self.request_with_retry(
            Method::PATCH,
            &["/drives/", &format!("drive{drive_id}")].concat(),
            body,
        )
        .await?;
        Ok(())
    }

    pub(crate) async fn add_net_device(
        &mut self,
        config: &NetworkConfig,
        device_id: String,
    ) -> Result<()> {
        let g_mac = match &config.guest_mac {
            Some(mac) => Some(MacAddr(mac.0)),
            None => None,
        };
        let body: String = json!({
            "iface_id": &device_id,
            "guest_mac": g_mac,
            "host_dev_name": &config.host_dev_name

        })
        .to_string();
        self.request_with_retry(
            Method::PUT,
            &["/network-interfaces/", &device_id].concat(),
            body,
        )
        .await?;
        Ok(())
    }

    pub(crate) async fn request_with_retry(
        &self,
        method: Method,
        uri: &str,
        data: String,
    ) -> Result<()> {
        let url: hyper::Uri = Uri::new(&self.asock_path, uri).into();
        self.send_request_with_retry(method, url, data).await
    }

    pub(crate) async fn send_request_with_retry(
        &self,
        method: Method,
        uri: hyper::Uri,
        data: String,
    ) -> Result<()> {
        debug!(sl(), "METHOD: {:?}", method.clone());
        debug!(sl(), "URI: {:?}", uri.clone());
        debug!(sl(), "DATA: {:?}", data.clone());
        let mut last_error = None;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
        for _count in 0..REQUEST_RETRY {
            let req = Request::builder()
                .method(method.clone())
                .uri(uri.clone())
                .header("Accept", "application/json")
                .header("Content-Type", "application/json")
                .body(Full::new(Bytes::from(data.clone())))?;

            let response = tokio::time::timeout_at(deadline, self.send_request(req)).await;
            let response = response.map_err(|_| anyhow!("Firecracker API deadline exceeded"))?;
            match response {
                Ok(resp) => {
                    debug!(sl(), "Request sent, resp: {:?}", resp);
                    return Ok(());
                }
                Err(resp) => {
                    debug!(sl(), "Request sent with error, resp: {:?}", resp);
                    last_error = Some(resp);
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                    continue;
                }
            }
        }
        Err(last_error.unwrap_or_else(|| anyhow!("Firecracker request was not attempted"))).context(
            format!("Firecracker request failed after {REQUEST_RETRY} attempts"),
        )
    }

    pub(crate) async fn send_request(
        &self,
        req: Request<Full<Bytes>>,
    ) -> Result<Response<Incoming>> {
        let resp = self.client.request(req).await?;

        let status = resp.status();
        debug!(sl(), "Request RESPONSE {:?} {:?}", &status, resp);
        if status.is_success() {
            Ok(resp)
        } else {
            let mut incoming = resp.into_body();
            let mut bytes = Vec::new();
            while let Some(frame) = incoming.frame().await {
                if let Ok(data) = frame?.into_data() {
                    anyhow::ensure!(
                        data.len() <= MAX_ERROR_BODY.saturating_sub(bytes.len()),
                        "Firecracker API error body exceeds limit"
                    );
                    bytes.extend_from_slice(&data);
                }
            }
            let body = String::from_utf8_lossy(&bytes);
            Err(anyhow!("Firecracker API returned HTTP {status}: {body}"))
        }
    }

    pub(crate) fn cleanup_resource(&self) {
        let Some(paths) = self.cleanup_resource_paths() else {
            return;
        };
        for path in paths {
            nix::mount::umount2(path.as_str(), nix::mount::MntFlags::MNT_DETACH).ok();
        }
        std::fs::remove_dir_all(self.vm_path.as_str())
            .inspect_err(|err| {
                error!(
                    sl(),
                    "failed to remove dir all for {} with error: {:?}", &self.vm_path, &err
                )
            })
            .ok();
    }

    fn cleanup_resource_paths(&self) -> Option<Vec<String>> {
        // Preparation may fail before assigning the jail directory. Never turn
        // that empty path into host /root mount targets during teardown.
        if self.vm_path.is_empty() {
            return None;
        }
        let root = [self.vm_path.as_str(), ROOT].join("/");
        let mut paths = vec![
            format!("{root}/{FC_KERNEL}"),
            format!("{root}/{FC_ROOT_FS}"),
        ];
        for i in 1..DISK_POOL_SIZE {
            paths.push(format!("{root}/{DRIVE_PREFIX}{i}"));
        }
        paths.push(root);
        Some(paths)
    }
}

#[cfg(test)]
mod error_tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn oversized_api_error_body_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fc.sock");
        let listener = tokio::net::UnixListener::bind(&path).unwrap();
        let peer = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                request.push(stream.read_u8().await.unwrap());
            }
            let body = vec![b'x'; MAX_ERROR_BODY + 1];
            let header = format!(
                "HTTP/1.1 400 Bad Request\r\nContent-Length: {}\r\n\r\n",
                body.len()
            );
            stream.write_all(header.as_bytes()).await.unwrap();
            let _ = stream.write_all(&body).await;
        });
        let (tx, _) = tokio::sync::mpsc::channel(1);
        let fc = FcInner::new(tx);
        let request = Request::builder()
            .uri(hyper::Uri::from(Uri::new(&path, "/")))
            .body(Full::new(Bytes::new()))
            .unwrap();
        assert!(fc
            .send_request(request)
            .await
            .unwrap_err()
            .to_string()
            .contains("exceeds limit"));
        peer.await.unwrap();
    }

    #[tokio::test]
    async fn api_error_body_survives_retries() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fc.sock");
        let listener = tokio::net::UnixListener::bind(&path).unwrap();
        let task = tokio::spawn(async move {
            for _ in 0..REQUEST_RETRY {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    request.push(stream.read_u8().await.unwrap());
                }
                let body = "{\"fault_message\":\"invalid drive path\"}";
                let response = format!(
                    "HTTP/1.1 400 Bad Request\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(),
                    body
                );
                stream.write_all(response.as_bytes()).await.unwrap();
            }
        });
        let (tx, _) = tokio::sync::mpsc::channel(1);
        let fc = FcInner::new(tx);
        let uri = Uri::new(&path, "/drives/test").into();
        let error = fc
            .send_request_with_retry(Method::GET, uri, String::new())
            .await
            .unwrap_err();
        let text = format!("{error:#}");
        assert!(text.contains("400"), "{}", text);
        assert!(text.contains("invalid drive path"), "{}", text);
        task.await.unwrap();
    }
}

#[cfg(test)]
mod cleanup_tests {
    use super::*;

    #[tokio::test]
    async fn unprepared_cleanup_has_no_host_targets() {
        let (tx, _) = tokio::sync::mpsc::channel(1);
        let mut fc = FcInner::new(tx);
        assert_eq!(fc.cleanup_resource_paths(), None);
        fc.cleanup().await.unwrap();

        assert!(fc.prepare_vm("unsupported", None, None).await.is_err());
        assert_eq!(fc.cleanup_resource_paths(), None);
        fc.cleanup().await.unwrap();
    }

    #[test]
    fn prepared_cleanup_targets_stay_inside_the_jail() {
        let (tx, _) = tokio::sync::mpsc::channel(1);
        let mut fc = FcInner::new(tx);
        fc.vm_path = "/run/kata/firecracker/test".into();
        assert_eq!(
            fc.cleanup_resource_paths().unwrap(),
            [
                "/run/kata/firecracker/test/root/vmlinux",
                "/run/kata/firecracker/test/root/rootfs",
                "/run/kata/firecracker/test/root/drive1",
                "/run/kata/firecracker/test/root/drive2",
                "/run/kata/firecracker/test/root/drive3",
                "/run/kata/firecracker/test/root/drive4",
                "/run/kata/firecracker/test/root/drive5",
                "/run/kata/firecracker/test/root",
            ]
        );
    }
}

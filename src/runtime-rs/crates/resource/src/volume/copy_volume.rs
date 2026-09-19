// Copyright (c) 2019-2022 Alibaba Cloud
// Copyright (c) 2019-2022 Ant Group
//
// SPDX-License-Identifier: Apache-2.0
//

use std::{
    collections::VecDeque,
    fs::File,
    io::Read,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use agent::Agent;
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use hypervisor::device::device_manager::DeviceManager;
use inotify::{EventMask, Inotify, WatchMask};
use kata_sys_util::mount::{get_mount_path, get_mount_type};
use nix::sys::stat::SFlag;
use rand::rng;
use rand::Rng;
use tokio::{io::AsyncReadExt, sync::RwLock, task::JoinHandle, time::Instant};
use walkdir::WalkDir;

use super::Volume;
use crate::guest_paths::DEFAULT_KATA_GUEST_SHARE_DIR;
use kata_types::{
    k8s::{is_configmap, is_downward_api, is_projected, is_secret},
    mount,
};
use oci_spec::runtime as oci;

const SYS_MOUNT_PREFIX: [&str; 2] = ["/proc", "/sys"];
const MONITOR_INTERVAL: Duration = Duration::from_millis(100);
const DEBOUNCE_TIME: Duration = Duration::from_millis(500);

// Copy host files into the guest and bind the guest copy into the container.
// Ignore /dev, directories and all other device files. We handle
// only regular files in /dev. It does not make sense to pass the host
// device nodes to the guest.
// skip the volumes whose source had already set to guest share dir.
pub(crate) struct CopyVolume {
    mounts: Vec<oci::Mount>,
    // Each copy has a distinct guest destination, even when host sources match.
    monitor_task: Option<JoinHandle<()>>,
}

struct FsWatcher {
    inotify: Inotify,
}

impl FsWatcher {
    fn new(source_path: &Path) -> Result<Self> {
        let inotify = Inotify::init()?;
        let walker = WalkDir::new(source_path)
            .follow_links(false)
            .into_iter()
            .filter_entry(|entry| {
                !entry
                    .file_name()
                    .to_str()
                    .map(|name| name.starts_with('.'))
                    .unwrap_or(false)
            });

        for entry in walker.filter_map(|entry| entry.ok()) {
            if entry.file_type().is_dir() {
                inotify.watches().add(
                    entry.path(),
                    WatchMask::CREATE
                        | WatchMask::DELETE
                        | WatchMask::MODIFY
                        | WatchMask::MOVED_FROM
                        | WatchMask::MOVED_TO
                        | WatchMask::CLOSE_WRITE,
                )?;
            }
        }

        Ok(Self { inotify })
    }

    fn start_monitor(
        mut self,
        agent: Arc<dyn Agent>,
        src: PathBuf,
        dst: PathBuf,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut buffer = [0u8; 4096];
            let mut last_event_time = None;

            // Cover changes between the initial copy and watcher installation.
            if let Err(err) = copy_dir_recursively(&src, &dst.to_string_lossy(), &agent).await {
                error!(sl!(), "Initial sync failed: {:?}", err);
            }

            loop {
                match self.inotify.read_events(&mut buffer) {
                    Ok(events) => {
                        for event in events {
                            if !event.mask.intersects(
                                EventMask::CREATE
                                    | EventMask::MODIFY
                                    | EventMask::DELETE
                                    | EventMask::MOVED_FROM
                                    | EventMask::MOVED_TO
                                    | EventMask::CLOSE_WRITE,
                            ) {
                                continue;
                            }
                            if let Some(name) = event.name {
                                info!(sl!(), "volume event {:?}: {:?}", event.mask, src.join(name));
                                last_event_time = Some(Instant::now());
                            }
                        }
                    }
                    Err(err) => eprintln!("inotify error: {err}"),
                }

                // Every event triggers a full copy, so no per-path event state is needed.
                if let Some(last_event) = last_event_time {
                    if last_event.elapsed() > DEBOUNCE_TIME {
                        if let Err(err) =
                            copy_dir_recursively(&src, &dst.to_string_lossy(), &agent).await
                        {
                            error!(sl!(), "copyfile {:?} -> {:?} failed: {:?}", &src, &dst, err);
                        }
                        last_event_time = None;
                    }
                }
                tokio::time::sleep(MONITOR_INTERVAL).await;
            }
        })
    }
}

impl CopyVolume {
    pub(crate) async fn new(m: &oci::Mount, cid: &str, agent: Arc<dyn Agent>) -> Result<Self> {
        let source_path = get_mount_path(m.source());
        let mut volume = Self {
            mounts: vec![],
            monitor_task: None,
        };

        let src = match std::fs::canonicalize(&source_path) {
            Err(err) => {
                return Err(anyhow!(format!(
                    "failed to canonicalize file {} {:?}",
                    &source_path, err
                )))
            }
            Ok(src) => src,
        };

        // append oci::Mount structure to volume mounts
        let mut oci_mount = oci::Mount::default();
        oci_mount.set_destination(m.destination().clone());
        oci_mount.set_typ(Some("bind".to_string()));
        oci_mount.set_options(m.options().clone());

        // If the mount source is a file, we can copy it to the sandbox
        if src.is_file() {
            // Generate guest path
            let guest_path = generate_copy_file_guest_path(cid, m.destination())
                .context("generate path failed")?;
            // Copy a single file
            Self::copy_file_to_guest(&src, &guest_path, &agent)
                .await
                .context("copy file to guest")?;

            oci_mount.set_source(Some(PathBuf::from(&guest_path)));
            volume.mounts.push(oci_mount);
        } else if src.is_dir() {
            // We allow directory copying wildly
            // source path: "/var/lib/kubelet/pods/6dad7281-57ff-49e4-b844-c588ceabec16/volumes/kubernetes.io~projected/kube-api-access-8s2nl"
            info!(sl!(), "copying directory {:?} to guest", &src);

            let guest_path = generate_copy_file_guest_path(cid, m.destination())
                .context("generate path failed")?;

            // Create directory
            Self::copy_directory_to_guest(&src, &guest_path, &agent)
                .await
                .context("copy directory to guest")?;

            oci_mount.set_source(Some(PathBuf::from(&guest_path)));
            volume.mounts.push(oci_mount);

            if is_watchable_volume(&src) {
                volume.monitor_task = Some(FsWatcher::new(&src)?.start_monitor(
                    agent,
                    src,
                    PathBuf::from(&guest_path),
                ));
            }
        } else {
            // If not, we can ignore it. Let's issue a warning so that the user knows.
            warn!(
                sl!(),
                "Ignoring non-regular file as FS sharing not supported. mount: {:?}", m
            );
        }

        Ok(volume)
    }

    async fn copy_file_to_guest(
        src: &Path,
        guest_path: &str,
        agent: &Arc<dyn Agent>,
    ) -> Result<()> {
        // Read file metadata
        let file_metadata = std::fs::metadata(src)
            .with_context(|| format!("Failed to read metadata from file: {src:?}"))?;

        // Open file
        let mut file = File::open(src).with_context(|| format!("Failed to open file: {src:?}"))?;

        // Open read file contents to buffer
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer)
            .with_context(|| format!("Failed to read file: {src:?}"))?;

        // Create gRPC request
        let r = agent::CopyFileRequest {
            path: guest_path.to_owned(),
            file_size: file_metadata.len() as i64,
            uid: file_metadata.uid() as i32,
            gid: file_metadata.gid() as i32,
            file_mode: file_metadata.mode(),
            data: buffer,
            ..Default::default()
        };

        debug!(sl!(), "copy_file: {:?} to sandbox {:?}", &src, guest_path);

        // Issue gRPC request to agent
        agent.copy_file(r).await.with_context(|| {
            format!("copy file request failed: src: {src:?}, dest: {guest_path:?}")
        })?;
        Ok(())
    }

    async fn copy_directory_to_guest(
        src: &Path,
        guest_path: &str,
        agent: &Arc<dyn Agent>,
    ) -> Result<()> {
        // create directory
        let dir_metadata =
            std::fs::metadata(src).context(format!("read metadata from directory: {src:?}"))?;

        // ttRPC request for creating directory
        let dir_request = agent::CopyFileRequest {
            path: guest_path.to_owned(),
            file_size: 0, // useless for dir
            uid: dir_metadata.uid() as i32,
            gid: dir_metadata.gid() as i32,
            file_mode: dir_metadata.mode(),
            data: vec![], // no files
            ..Default::default()
        };

        info!(
            sl!(),
            "creating directory: {:?} in sandbox with file_mode: {:?}",
            guest_path,
            dir_request.file_mode
        );

        // send request for creating directory
        agent
            .copy_file(dir_request)
            .await
            .context(format!("create directory in sandbox: {guest_path:?}"))?;

        // recursively copy files from this directory
        // similar to `scp -r $source_dir $target_dir`
        copy_dir_recursively(src, guest_path, agent)
            .await
            .context(format!("failed to copy directory contents: {src:?}"))?;

        Ok(())
    }
}

#[async_trait]
impl Volume for CopyVolume {
    fn get_volume_mount(&self) -> anyhow::Result<Vec<oci::Mount>> {
        Ok(self.mounts.clone())
    }

    fn get_storage(&self) -> Result<Vec<agent::Storage>> {
        Ok(vec![])
    }

    async fn cleanup(&self, _device_manager: &RwLock<DeviceManager>) -> Result<()> {
        self.stop_monitor();
        // Copied files live until sandbox teardown; no host mount to unmount.
        Ok(())
    }
}

impl CopyVolume {
    fn stop_monitor(&self) {
        if let Some(task) = &self.monitor_task {
            task.abort();
        }
    }
}

impl Drop for CopyVolume {
    fn drop(&mut self) {
        self.stop_monitor();
    }
}

async fn copy_dir_recursively<P: AsRef<Path>>(
    src_dir: P,
    dest_dir: &str,
    agent: &Arc<dyn Agent>,
) -> Result<()> {
    let mut queue = VecDeque::new();
    queue.push_back((src_dir.as_ref().to_path_buf(), dest_dir.to_string()));

    while let Some((current_src, current_dest)) = queue.pop_front() {
        let mut entries = tokio::fs::read_dir(&current_src)
            .await
            .context(format!("read directory: {current_src:?}"))?;

        while let Some(entry) = entries
            .next_entry()
            .await
            .context(format!("read directory entry in {current_src:?}"))?
        {
            let entry_path = entry.path();
            let file_name = entry_path
                .file_name()
                .ok_or_else(|| anyhow!("get file name for {:?}", entry_path))?
                .to_string_lossy()
                .to_string();

            let dest_path = format!("{current_dest}/{file_name}");

            let metadata = entry
                .metadata()
                .await
                .context(format!("read metadata for {entry_path:?}"))?;

            if metadata.is_symlink() {
                // handle symlinks
                let entry_path_err = entry_path.clone();
                let entry_path_clone = entry_path.clone();
                let link_target =
                    tokio::task::spawn_blocking(move || std::fs::read_link(&entry_path_clone))
                        .await
                        .context(format!(
                            "failed to spawn blocking task for symlink: {entry_path_err:?}"
                        ))??;

                let link_target_str = link_target.to_string_lossy().into_owned();
                let symlink_request = agent::CopyFileRequest {
                    path: dest_path.clone(),
                    file_size: link_target_str.len() as i64,
                    uid: metadata.uid() as i32,
                    gid: metadata.gid() as i32,
                    file_mode: SFlag::S_IFLNK.bits(),
                    data: link_target_str.clone().into_bytes(),
                    ..Default::default()
                };
                info!(
                    sl!(),
                    "copying symlink_request {:?} in sandbox with file_mode: {:?}",
                    dest_path.clone(),
                    symlink_request.file_mode
                );

                agent.copy_file(symlink_request).await.context(format!(
                    "failed to create symlink: {dest_path:?} -> {link_target_str:?}"
                ))?;
            } else if metadata.is_dir() {
                // handle directory
                let dir_request = agent::CopyFileRequest {
                    path: dest_path.clone(),
                    file_size: 0,
                    uid: metadata.uid() as i32,
                    gid: metadata.gid() as i32,
                    file_mode: metadata.mode(),
                    data: vec![],
                    ..Default::default()
                };
                info!(
                    sl!(),
                    "copying subdirectory {:?} in sandbox with file_mode: {:?}",
                    dir_request.path,
                    dir_request.file_mode
                );
                agent
                    .copy_file(dir_request)
                    .await
                    .context(format!("Failed to create subdirectory: {dest_path:?}"))?;

                // push back the sub-dir into queue to handle it in time
                queue.push_back((entry_path, dest_path));
            } else if metadata.is_file() {
                // async read file
                let mut file = tokio::fs::File::open(&entry_path)
                    .await
                    .context(format!("open file: {entry_path:?}"))?;

                let mut buffer = Vec::new();
                file.read_to_end(&mut buffer)
                    .await
                    .context(format!("read file: {entry_path:?}"))?;

                let file_request = agent::CopyFileRequest {
                    path: dest_path.clone(),
                    file_size: metadata.len() as i64,
                    uid: metadata.uid() as i32,
                    gid: metadata.gid() as i32,
                    file_mode: metadata.mode(),
                    data: buffer,
                    ..Default::default()
                };

                info!(sl!(), "copy file {:?} to guest", dest_path.clone());
                agent
                    .copy_file(file_request)
                    .await
                    .context(format!("copy file: {entry_path:?} -> {dest_path:?}"))?;
            }
        }
    }

    Ok(())
}

pub(crate) fn is_copy_volume(m: &oci::Mount) -> bool {
    let mount_type = get_mount_type(m);
    (mount_type == "bind" || mount_type == mount::KATA_EPHEMERAL_VOLUME_TYPE)
        && !is_host_device(&get_mount_path(&Some(m.destination().clone())))
        && !is_system_mount(&get_mount_path(m.source()))
}

fn is_host_device(dest: &str) -> bool {
    if dest == "/dev" {
        return true;
    }

    if dest.starts_with("/dev/") {
        let src = match std::fs::canonicalize(dest) {
            Err(_) => return false,
            Ok(src) => src,
        };

        if src.is_file() {
            return false;
        }

        return true;
    }

    false
}

// Skip mounting certain system paths("/sys/*", "/proc/*")
// from source on the host side into the container as it does not
// make sense to do so.
// Agent will support this kind of bind mount.
fn is_system_mount(src: &str) -> bool {
    for p in SYS_MOUNT_PREFIX {
        let sub_dir_p = format!("{p}/");
        if src == p || src.contains(sub_dir_p.as_str()) {
            return true;
        }
    }
    false
}

// Keep the device ID prefix in the generated mount name.
pub fn generate_mount_path(id: &str, file_name: &str) -> String {
    let mut nid = String::from(id);
    if nid.len() > 10 {
        nid = nid.chars().take(10).collect();
    }

    let mut uid = uuid::Uuid::new_v4().to_string();
    let uid_vec: Vec<&str> = uid.splitn(2, '-').collect();
    uid = String::from(uid_vec[0]);

    format!("{nid}-{uid}-{file_name}")
}

/// This function is used to check whether a given volume is a watchable volume.
/// More specifically, it determines whether the volume's path is located under
/// a predefined list of allowed copy directories.
pub(crate) fn is_watchable_volume(source_path: &PathBuf) -> bool {
    if !source_path.is_dir() {
        return false;
    }
    // watchable list: { kubernetes.io~projected, kubernetes.io~configmap, kubernetes.io~secret, kubernetes.io~downward-api }
    is_projected(source_path)
        || is_downward_api(source_path)
        || is_secret(source_path)
        || is_configmap(source_path)
}

/// Generates a destination for an agent CopyFile request.
///
/// CopyFile writes into the guest filesystem and confines destinations to the
/// fixed guest shared directory. Unlike filesystem-sharing paths, this guest
/// path must not be adjusted when rootless mode is enabled.
fn generate_copy_file_guest_path(cid: &str, mount_destination: &Path) -> Result<String> {
    let mut data = vec![0u8; 8];
    let mut rng = rng(); // Get a thread-local RNG
    rng.fill_bytes(&mut data);

    let hex_str = hex::encode(data);
    let dest_base = mount_destination
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| anyhow!("get mount destination failed"))?;

    Ok(format!(
        "{}{}-{}-{}",
        DEFAULT_KATA_GUEST_SHARE_DIR, cid, hex_str, dest_base
    ))
}

#[cfg(test)]
mod test {
    use super::*;

    #[tokio::test]
    async fn cleanup_only_stops_the_owned_volume_monitor() {
        fn monitored_volume() -> (
            CopyVolume,
            tokio::sync::mpsc::Sender<tokio::sync::oneshot::Sender<()>>,
        ) {
            let (sender, mut receiver) =
                tokio::sync::mpsc::channel::<tokio::sync::oneshot::Sender<()>>(1);
            let task = tokio::spawn(async move {
                while let Some(reply) = receiver.recv().await {
                    let _ = reply.send(());
                }
            });
            (
                CopyVolume {
                    mounts: vec![],
                    monitor_task: Some(task),
                },
                sender,
            )
        }

        // Two mounts of the same host source still own separate guest copies.
        let (first, first_monitor) = monitored_volume();
        let (second, second_monitor) = monitored_volume();
        let devices = RwLock::new(
            DeviceManager::new(Arc::new(hypervisor::firecracker::Firecracker::new()))
                .await
                .unwrap(),
        );

        first.cleanup(&devices).await.unwrap();
        tokio::time::timeout(Duration::from_secs(1), first_monitor.closed())
            .await
            .expect("first monitor must stop");
        first.cleanup(&devices).await.unwrap();
        drop(first);

        let (reply, received) = tokio::sync::oneshot::channel();
        second_monitor.send(reply).await.unwrap();
        tokio::time::timeout(Duration::from_secs(1), received)
            .await
            .expect("second monitor must keep running")
            .unwrap();

        // Also cancel if setup fails before the normal volume cleanup path.
        drop(second);
        tokio::time::timeout(Duration::from_secs(1), second_monitor.closed())
            .await
            .expect("dropping a volume must stop its monitor");
    }

    #[test]
    fn test_is_system_mount() {
        let sys_dir = "/sys";
        let proc_dir = "/proc";
        let sys_sub_dir = "/sys/fs/cgroup";
        let proc_sub_dir = "/proc/cgroups";
        let not_sys_dir = "/root";

        assert!(is_system_mount(sys_dir));
        assert!(is_system_mount(proc_dir));
        assert!(is_system_mount(sys_sub_dir));
        assert!(is_system_mount(proc_sub_dir));
        assert!(!is_system_mount(not_sys_dir));
    }

    #[test]
    fn test_is_watchable_volume() {
        // The configmap is /var/lib/kubelet/pods/<uid>/volumes/kubernetes.io~configmap/kube-configmap-0s2no/{..data, key1, key2,...}
        // The secret is /var/lib/kubelet/pods/<uid>/volumes/kubernetes.io~secret/kube-secret-2s2np/{..data, key1, key2,...}
        // The projected is /var/lib/kubelet/pods/<uid>/volumes/kubernetes.io~projected/kube-api-access-8s2nl/{..data, key1, key2,...}
        // The downward-api is /var/lib/kubelet/pods/<uid>/volumes/kubernetes.io~downward-api/downward-api-xxxx/{..data, key1, key2,...}
        let configmap =
            "var/lib/kubelet/pods/1000/volumes/kubernetes.io~configmap/kube-configmap-0s2no";
        let secret = "var/lib/kubelet/pods/1000/volumes/kubernetes.io~secret/kube-secret-2s2np";
        let projected =
            "var/lib/kubelet/1000/<uid>/volumes/kubernetes.io~projected/kube-api-access-8s2nl";
        let downward_api =
            "var/lib/kubelet/1000/<uid>/volumes/kubernetes.io~downward-api/downward-api-xxxx";

        let temp_dir = tempfile::tempdir().unwrap();
        let cm_path = temp_dir.path().join(configmap);
        std::fs::create_dir_all(&cm_path).unwrap();
        let secret_path = temp_dir.path().join(secret);
        std::fs::create_dir_all(&secret_path).unwrap();
        let projected_path = temp_dir.path().join(projected);
        std::fs::create_dir_all(&projected_path).unwrap();
        let downward_api_path = temp_dir.path().join(downward_api);
        std::fs::create_dir_all(&downward_api_path).unwrap();

        assert!(is_watchable_volume(&cm_path));
        assert!(is_watchable_volume(&secret_path));
        assert!(is_watchable_volume(&projected_path));
        assert!(is_watchable_volume(&downward_api_path));
    }

    #[test]
    fn test_generate_copy_file_guest_path() {
        let path =
            generate_copy_file_guest_path("sandbox-id", Path::new("/etc/resolv.conf")).unwrap();

        assert!(path.starts_with(&format!("{DEFAULT_KATA_GUEST_SHARE_DIR}sandbox-id-")));
        assert!(path.ends_with("-resolv.conf"));
    }
}

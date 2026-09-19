// Copyright (c) 2019 Ant Financial
//
// SPDX-License-Identifier: Apache-2.0
//

use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::fmt::{Debug, Formatter};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, RwLock};

use anyhow::{anyhow, Context, Result};
use libc::pid_t;
use rustjail::cgroups::DevicesCgroupInfo;
use rustjail::container::BaseContainer;
use rustjail::container::LinuxContainer;
use rustjail::process::Process;
use slog::Logger;
use thiserror::Error;
use tokio::sync::mpsc::{channel, Receiver, Sender};
use tokio::sync::oneshot;
use tokio::sync::Mutex;

use crate::mount::{get_mount_fs_type, TYPE_ROOTFS};
use crate::namespace::Namespace;
use crate::netlink::Handle;
use crate::storage::StorageDevice;
use crate::uevent::{Uevent, UeventMatcher};

/// Errors that can occur when looking up processes in the sandbox.
#[derive(Debug, Error)]
pub enum SandboxError {
    #[error("Invalid container id")]
    InvalidContainerId,
    #[error("Process not found: init process missing")]
    InitProcessNotFound,
    #[error("Process not found: invalid exec id")]
    InvalidExecId,
}

type UeventWatcher = (Box<dyn UeventMatcher>, oneshot::Sender<Uevent>);

#[derive(Clone)]
pub struct StorageState {
    count: Arc<AtomicU32>,
    device: Arc<StorageDevice>,

    /// Whether the storage is shared across multiple containers (e.g.
    /// block-based emptyDirs). Shared storages should not be cleaned up
    /// when a container exits; cleanup happens only when the sandbox is
    /// destroyed.
    shared: bool,
}

impl Debug for StorageState {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StorageState").finish()
    }
}

impl StorageState {
    #[tracing::instrument(skip_all)]
    fn new(shared: bool) -> Self {
        StorageState {
            count: Arc::new(AtomicU32::new(1)),
            device: Arc::new(StorageDevice::default()),
            shared,
        }
    }

    pub fn path(&self) -> Option<&str> {
        self.device.path()
    }

    pub fn is_shared(&self) -> bool {
        self.shared
    }

    pub async fn ref_count(&self) -> u32 {
        self.count.load(Ordering::Relaxed)
    }

    async fn inc_ref_count(&self) {
        self.count.fetch_add(1, Ordering::Acquire);
    }

    async fn dec_and_test_ref_count(&self) -> bool {
        self.count.fetch_sub(1, Ordering::AcqRel) == 1
    }
}

#[derive(Debug)]
pub struct Sandbox {
    pub logger: Logger,
    pub id: String,
    pub hostname: String,
    pub containers: HashMap<String, LinuxContainer>,
    pub container_mounts: HashMap<String, Vec<String>>,
    pub uevent_map: HashMap<String, Uevent>,
    pub uevent_watchers: Vec<Option<UeventWatcher>>,
    pub shared_utsns: Namespace,
    pub shared_ipcns: Namespace,
    pub sandbox_pidns: Option<Namespace>,
    pub storages: HashMap<String, StorageState>,
    pub no_pivot_root: bool,
    pub sender: Option<tokio::sync::oneshot::Sender<i32>>,
    pub rtnl: Handle,
    pub event_rx: Arc<Mutex<Receiver<String>>>,
    pub event_tx: Option<Sender<String>>,
    pub devcg_info: Arc<RwLock<DevicesCgroupInfo>>,
}

impl Sandbox {
    #[tracing::instrument(skip_all)]
    pub fn new(logger: &Logger) -> Result<Self> {
        let fs_type = get_mount_fs_type("/")?;
        let logger = logger.new(o!("subsystem" => "sandbox"));
        let (tx, rx) = channel::<String>(100);
        let event_rx = Arc::new(Mutex::new(rx));

        Ok(Sandbox {
            logger: logger.clone(),
            id: String::new(),
            hostname: String::new(),
            containers: HashMap::new(),
            container_mounts: HashMap::new(),
            uevent_map: HashMap::new(),
            uevent_watchers: Vec::new(),
            shared_utsns: Namespace::new(&logger),
            shared_ipcns: Namespace::new(&logger),
            sandbox_pidns: None,
            storages: HashMap::new(),
            no_pivot_root: fs_type.eq(TYPE_ROOTFS),
            sender: None,
            rtnl: Handle::new()?,
            event_rx,
            event_tx: Some(tx),
            devcg_info: Arc::new(RwLock::new(DevicesCgroupInfo::default())),
        })
    }

    /// Add a new storage object or increase reference count of existing one.
    /// The caller may detect new storage object by checking `StorageState.refcount == 1`.
    /// The `shared` flag indicates if this storage is shared across multiple containers;
    /// if true, cleanup will be skipped when containers exit.

    #[tracing::instrument(skip_all)]
    pub async fn add_sandbox_storage(&mut self, path: &str, shared: bool) -> StorageState {
        match self.storages.entry(path.to_string()) {
            Entry::Occupied(e) => {
                let state = e.get().clone();
                state.inc_ref_count().await;
                state
            }
            Entry::Vacant(e) => {
                let state = StorageState::new(shared);
                e.insert(state.clone());
                state
            }
        }
    }

    /// Update the storage device associated with a path.
    /// Preserves the existing shared flag and reference count.
    pub fn update_sandbox_storage(
        &mut self,
        path: &str,
        device: Arc<StorageDevice>,
    ) -> std::result::Result<Arc<StorageDevice>, Arc<StorageDevice>> {
        match self.storages.get(path) {
            None => Err(device),
            Some(existing) => {
                let state = StorageState {
                    device,
                    ..existing.clone()
                };
                // Safe to unwrap() because we have just ensured existence of entry via get().
                let state = self.storages.insert(path.to_string(), state).unwrap();
                Ok(state.device)
            }
        }
    }

    /// Decrease reference count and destroy the storage object if reference count reaches zero.
    ///
    /// For shared storages (e.g., emptyDir volumes), cleanup is skipped even when refcount
    /// reaches zero. The storage entry is kept in the map so subsequent containers can reuse
    /// the already-mounted storage. Actual cleanup happens when the sandbox is destroyed.
    ///
    /// Returns `Ok(true)` if the reference count has reached zero and the storage object has been
    /// removed.

    #[tracing::instrument(skip_all)]
    pub async fn remove_sandbox_storage(&mut self, path: &str) -> Result<bool> {
        match self.storages.get(path) {
            None => Err(anyhow!("Sandbox storage with path {} not found", path)),
            Some(state) => {
                if state.dec_and_test_ref_count().await {
                    if state.is_shared() {
                        state.count.store(1, Ordering::Release);
                        return Ok(false);
                    }
                    if let Some(storage) = self.storages.remove(path) {
                        storage.device.cleanup()?;
                    }
                    Ok(true)
                } else {
                    Ok(false)
                }
            }
        }
    }

    #[tracing::instrument(skip_all)]
    pub async fn setup_shared_namespaces(&mut self) -> Result<bool> {
        // Set up shared IPC namespace
        self.shared_ipcns = Namespace::new(&self.logger)
            .get_ipc()
            .setup()
            .await
            .context("setup persistent IPC namespace")?;

        // // Set up shared UTS namespace
        self.shared_utsns = Namespace::new(&self.logger)
            .get_uts(self.hostname.as_str())
            .setup()
            .await
            .context("setup persistent UTS namespace")?;

        Ok(true)
    }

    #[tracing::instrument(skip_all)]
    pub fn update_shared_pidns(&mut self, c: &LinuxContainer) -> Result<()> {
        // Populate the shared pid path only if this is an infra container and
        // sandbox_pidns has not been passed in the create_sandbox request.
        // This means a separate pause process has not been created. We treat the
        // first container created as the infra container in that case
        // and use its pid namespace in case pid namespace needs to be shared.
        if self.sandbox_pidns.is_none() && self.containers.is_empty() {
            let init_pid = c.init_process_pid;
            if init_pid == -1 {
                return Err(anyhow!(
                    "Failed to setup pid namespace: init container pid is -1"
                ));
            }

            let mut pid_ns = Namespace::new(&self.logger).get_pid();
            pid_ns.path = format!("/proc/{init_pid}/ns/pid");

            self.sandbox_pidns = Some(pid_ns);
        }

        Ok(())
    }

    pub fn add_container(&mut self, c: LinuxContainer) {
        self.containers.insert(c.id.clone(), c);
    }

    pub fn get_container(&mut self, id: &str) -> Option<&mut LinuxContainer> {
        self.containers.get_mut(id)
    }

    pub fn find_process(&mut self, pid: pid_t) -> Option<&mut Process> {
        for (_, c) in self.containers.iter_mut() {
            for p in c.processes.values_mut() {
                if p.pid == pid {
                    return Some(p);
                }
            }
        }

        None
    }

    pub fn find_container_process(
        &mut self,
        cid: &str,
        eid: &str,
    ) -> Result<&mut Process, SandboxError> {
        let ctr = self
            .get_container(cid)
            .ok_or(SandboxError::InvalidContainerId)?;

        if eid.is_empty() {
            let init_pid = ctr.init_process_pid;
            return ctr
                .processes
                .values_mut()
                .find(|p| p.pid == init_pid)
                .ok_or(SandboxError::InitProcessNotFound);
        }

        ctr.get_process(eid)
            .map_err(|_| SandboxError::InvalidExecId)
    }

    #[tracing::instrument(skip_all)]
    pub async fn destroy(&mut self) -> Result<()> {
        for ctr in self.containers.values_mut() {
            ctr.destroy().await?;
        }
        Ok(())
    }

    #[tracing::instrument(skip_all)]
    pub async fn run_oom_event_monitor(&self, mut rx: Receiver<String>, container_id: String) {
        let logger = self.logger.clone();
        let tx = match self.event_tx.as_ref() {
            Some(v) => v.clone(),
            None => {
                error!(
                    logger,
                    "sandbox.event_tx not found in run_oom_event_monitor"
                );
                return;
            }
        };

        tokio::spawn(async move {
            loop {
                let event = rx.recv().await;
                // None means the container has exited, and sender in OOM notifier is dropped.
                if event.is_none() {
                    return;
                }
                info!(logger, "got an OOM event {:?}", event);
                if let Err(e) = tx.send(container_id.clone()).await {
                    error!(logger, "failed to send message: {:?}", e);
                }
            }
        });
    }
}

#[cfg(test)]
#[allow(dead_code)]
#[allow(unused_imports)]
mod tests {
    use super::*;
    use crate::mount::baremount;
    use anyhow::{anyhow, Error};
    use nix::mount::MsFlags;
    use oci::{Linux, LinuxBuilder, LinuxDeviceCgroup, LinuxResources, Root, Spec, SpecBuilder};
    use oci_spec::runtime as oci;
    use rustjail::container::LinuxContainer;
    use rustjail::process::Process;
    use rustjail::specconv::CreateOpts;
    use slog::Logger;
    use std::fs::{self, File};
    use std::io::prelude::*;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};
    use tempfile::{tempdir, Builder, TempDir};
    use test_utils::skip_if_not_root;

    const CGROUP_PARENT: &str = "kata.agent.test.k8s.io";

    fn bind_mount(src: &str, dst: &str, logger: &Logger) -> Result<(), Error> {
        let src_path = Path::new(src);
        let dst_path = Path::new(dst);

        baremount(src_path, dst_path, "bind", MsFlags::MS_BIND, "", logger)
    }

    use serial_test::serial;

    #[tokio::test]
    #[serial]
    async fn set_sandbox_storage() {
        let logger = slog::Logger::root(slog::Discard, o!());
        let mut s = Sandbox::new(&logger).unwrap();

        let tmpdir = Builder::new().tempdir().unwrap();
        let tmpdir_path = tmpdir.path().to_str().unwrap();

        // Add a new sandbox storage
        let new_storage = s.add_sandbox_storage(tmpdir_path, false).await;

        // Check the reference counter
        let ref_count = new_storage.ref_count().await;
        assert_eq!(
            ref_count, 1,
            "Invalid refcount, got {ref_count} expected 1."
        );

        // Use the existing sandbox storage
        let new_storage = s.add_sandbox_storage(tmpdir_path, false).await;

        // Since we are using existing storage, the reference counter
        // should be 2 by now.
        let ref_count = new_storage.ref_count().await;
        assert_eq!(
            ref_count, 2,
            "Invalid refcount, got {ref_count} expected 2."
        );
    }

    #[tokio::test]
    #[serial]
    async fn unset_and_remove_sandbox_storage() {
        skip_if_not_root!();

        let logger = slog::Logger::root(slog::Discard, o!());
        let mut s = Sandbox::new(&logger).unwrap();

        assert!(
            s.remove_sandbox_storage("/tmp/testEphePath").await.is_err(),
            "Should fail because sandbox storage doesn't exist"
        );

        let tmpdir = Builder::new().tempdir().unwrap();
        let tmpdir_path = tmpdir.path().to_str().unwrap();

        let srcdir = Builder::new()
            .prefix("src")
            .tempdir_in(tmpdir_path)
            .unwrap();
        let srcdir_path = srcdir.path().to_str().unwrap();

        let destdir = Builder::new()
            .prefix("dest")
            .tempdir_in(tmpdir_path)
            .unwrap();
        let destdir_path = destdir.path().to_str().unwrap();

        assert!(bind_mount(srcdir_path, destdir_path, &logger).is_ok());

        s.add_sandbox_storage(destdir_path, false).await;
        let storage = StorageDevice::new(destdir_path.to_string());
        assert!(s
            .update_sandbox_storage(destdir_path, Arc::new(storage))
            .is_ok());
        assert!(s.remove_sandbox_storage(destdir_path).await.is_ok());

        let other_dir_str;
        {
            // Create another folder in a separate scope to ensure that is
            // deleted
            let other_dir = Builder::new()
                .prefix("dir")
                .tempdir_in(tmpdir_path)
                .unwrap();
            let other_dir_path = other_dir.path().to_str().unwrap();
            other_dir_str = other_dir_path.to_string();

            s.add_sandbox_storage(other_dir_path, false).await;
            let storage = StorageDevice::new(other_dir_path.to_string());
            assert!(s
                .update_sandbox_storage(other_dir_path, Arc::new(storage))
                .is_ok());
        }

        assert!(s.remove_sandbox_storage(&other_dir_str).await.is_ok());
    }

    #[tokio::test]
    #[serial]
    async fn unset_sandbox_storage() {
        let logger = slog::Logger::root(slog::Discard, o!());
        let mut s = Sandbox::new(&logger).unwrap();

        let storage_path = "/tmp/testEphe";

        // Add a new sandbox storage
        s.add_sandbox_storage(storage_path, false).await;
        // Use the existing sandbox storage
        let state = s.add_sandbox_storage(storage_path, false).await;
        assert!(
            state.ref_count().await > 1,
            "Expects false as the storage is not new."
        );

        assert!(
            !s.remove_sandbox_storage(storage_path).await.unwrap(),
            "Expects false as there is still a storage."
        );

        // Reference counter should decrement to 1.
        let storage = &s.storages[storage_path];
        let refcount = storage.ref_count().await;
        assert_eq!(refcount, 1, "Invalid refcount, got {refcount} expected 1.");

        assert!(
            s.remove_sandbox_storage(storage_path).await.unwrap(),
            "Expects true as there is still a storage."
        );

        // Since no container is using this sandbox storage anymore
        // there should not be any reference in sandbox struct
        // for the given storage
        assert!(
            !s.storages.contains_key(storage_path),
            "The storages map should not contain the key {}",
            storage_path
        );

        // If no container is using the sandbox storage, the reference
        // counter for it should not exist.
        assert!(
            s.remove_sandbox_storage(storage_path).await.is_err(),
            "Expects false as the reference counter should no exist."
        );
    }

    fn create_dummy_opts() -> CreateOpts {
        let start = SystemTime::now();
        let since_the_epoch = start
            .duration_since(UNIX_EPOCH)
            .expect("Time went backwards");

        let mut root = Root::default();
        root.set_path(PathBuf::from("/"));

        let mut cgroup = LinuxDeviceCgroup::default();
        cgroup.set_allow(true);
        cgroup.set_access(Some(String::from("rwm")));

        let mut linux_resources = LinuxResources::default();
        linux_resources.set_devices(Some(vec![cgroup]));

        let cgroups_path = format!(
            "/{}/dummycontainer{}",
            CGROUP_PARENT,
            since_the_epoch.as_micros()
        );

        let spec = SpecBuilder::default()
            .linux(
                LinuxBuilder::default()
                    .cgroups_path(cgroups_path)
                    .resources(linux_resources)
                    .build()
                    .unwrap(),
            )
            .root(root)
            .build()
            .unwrap();

        CreateOpts {
            no_pivot_root: false,
            spec: Some(spec),
        }
    }

    fn create_linuxcontainer() -> (LinuxContainer, TempDir) {
        // Create a temporal directory
        let dir = tempdir()
            .map_err(|e| anyhow!(e).context("tempdir failed"))
            .unwrap();

        let container = LinuxContainer::new(
            "some_id",
            dir.path().join("rootfs").to_str().unwrap(),
            None,
            create_dummy_opts(),
            &slog_scope::logger(),
        )
        .unwrap();

        // Create a new container
        (container, dir)
    }

    #[tokio::test]
    #[serial]
    async fn get_container_entry_exist() {
        skip_if_not_root!();

        let logger = slog::Logger::root(slog::Discard, o!());
        let mut s = Sandbox::new(&logger).unwrap();
        let (linux_container, _root) = create_linuxcontainer();

        s.containers
            .insert("testContainerID".to_string(), linux_container);
        let cnt = s.get_container("testContainerID");
        assert!(cnt.is_some());
    }

    #[tokio::test]
    #[serial]
    async fn get_container_no_entry() {
        let logger = slog::Logger::root(slog::Discard, o!());
        let mut s = Sandbox::new(&logger).unwrap();

        let cnt = s.get_container("testContainerID");
        assert!(cnt.is_none());
    }

    #[tokio::test]
    #[serial]
    #[cfg(not(target_arch = "powerpc64"))]
    async fn add_and_get_container() {
        skip_if_not_root!();

        let logger = slog::Logger::root(slog::Discard, o!());
        let mut s = Sandbox::new(&logger).unwrap();
        let (linux_container, _root) = create_linuxcontainer();

        s.add_container(linux_container);
        assert!(s.get_container("some_id").is_some());
    }

    #[tokio::test]
    #[serial]
    #[tracing::instrument(skip_all)]
    async fn update_shared_pidns() {
        skip_if_not_root!();

        let logger = slog::Logger::root(slog::Discard, o!());
        let mut s = Sandbox::new(&logger).unwrap();
        let test_pid = 9999;

        let (mut linux_container, _root) = create_linuxcontainer();
        linux_container.init_process_pid = test_pid;

        s.update_shared_pidns(&linux_container).unwrap();

        assert!(s.sandbox_pidns.is_some());

        let ns_path = format!("/proc/{test_pid}/ns/pid");
        assert_eq!(s.sandbox_pidns.unwrap().path, ns_path);
    }

    #[tokio::test]
    #[serial]
    async fn test_sandbox_set_destroy() {
        let logger = slog::Logger::root(slog::Discard, o!());
        let mut s = Sandbox::new(&logger).unwrap();
        let ret = s.destroy().await;
        assert!(ret.is_ok());
    }

    #[tokio::test]
    #[cfg(not(target_arch = "powerpc64"))]
    async fn test_find_container_process() {
        skip_if_not_root!();

        let logger = slog::Logger::root(slog::Discard, o!());
        let mut s = Sandbox::new(&logger).unwrap();
        let cid = "container-123";

        let (mut linux_container, _root) = create_linuxcontainer();
        linux_container.init_process_pid = 1;
        linux_container.id = cid.to_string();
        // add init process
        let mut init_process =
            Process::new(&logger, &oci::Process::default(), "1", true, 1).unwrap();
        init_process.pid = 1;
        linux_container
            .processes
            .insert("1".to_string(), init_process);
        // add exec process
        let mut exec_process =
            Process::new(&logger, &oci::Process::default(), "exec-123", false, 1).unwrap();
        exec_process.pid = 123;
        linux_container
            .processes
            .insert("exec-123".to_string(), exec_process);

        s.add_container(linux_container);

        // empty exec-id will return init process
        let p = s.find_container_process(cid, "");
        assert!(p.is_ok(), "Expecting Ok, Got {:?}", p);
        let p = p.unwrap();
        assert_eq!("1", p.exec_id, "exec_id should be 1");
        assert!(p.init, "init flag should be true");

        // get exist exec-id will return the exec process
        let p = s.find_container_process(cid, "exec-123");
        assert!(p.is_ok(), "Expecting Ok, Got {:?}", p);
        let p = p.unwrap();
        assert_eq!("exec-123", p.exec_id, "exec_id should be exec-123");
        assert!(!p.init, "init flag should be false");

        // get not exist exec-id will return error
        let p = s.find_container_process(cid, "exec-456");
        assert!(p.is_err(), "Expecting Error, Got {:?}", p);

        // container does not exist
        let p = s.find_container_process("not-exist-cid", "");
        assert!(p.is_err(), "Expecting Error, Got {:?}", p);
    }

    #[tokio::test]
    #[cfg(not(target_arch = "powerpc64"))]
    async fn test_find_process() {
        skip_if_not_root!();

        let logger = slog::Logger::root(slog::Discard, o!());

        let test_pids = [i32::MIN, -1, 0, 1, i32::MAX];

        for test_pid in test_pids {
            let mut s = Sandbox::new(&logger).unwrap();
            let (mut linux_container, _root) = create_linuxcontainer();

            let mut test_process = Process::new(
                &logger,
                &oci::Process::default(),
                "this_is_a_test_process",
                true,
                1,
            )
            .unwrap();
            // processes interally only have pids when manually set
            test_process.pid = test_pid;
            let test_exec_id = test_process.exec_id.clone();
            linux_container.processes.insert(test_exec_id, test_process);

            s.add_container(linux_container);

            let find_result = s.find_process(test_pid);

            // test first if it finds anything
            assert!(find_result.is_some(), "Should be able to find a process");

            let found_process = find_result.unwrap();

            // then test if it founds the correct process
            assert_eq!(
                found_process.pid, test_pid,
                "Should be able to find correct process"
            );
        }

        // to test for nonexistent pids, any pid that isn't the one set
        // above should work, as linuxcontainer starts with no processes
        let mut s = Sandbox::new(&logger).unwrap();

        let nonexistent_test_pid = 1234;

        let find_result = s.find_process(nonexistent_test_pid);

        assert!(
            find_result.is_none(),
            "Shouldn't find a process for non existent pid"
        );
    }
}

// Copyright (c) 2019, 2020 Ant Group
//
// SPDX-License-Identifier: Apache-2.0
//

use anyhow::{anyhow, Context, Result};
use libc::pid_t;
use oci::{Linux, LinuxDevice, LinuxIdMapping, LinuxNamespace, LinuxResources, Spec};
use oci_spec::runtime as oci;
use runtime_spec::ContainerState;
use std::clone::Clone;
use std::ffi::CString;
use std::fmt::Display;
use std::fs;
use std::os::unix::io::{AsFd, BorrowedFd, IntoRawFd, OwnedFd, RawFd};
use std::path::{Path, PathBuf};

use crate::cgroups_rs as cgroups;
use cgroups::freezer::FreezerState;

use crate::capabilities;
#[cfg(not(test))]
use crate::cgroups::fs::Manager as FsManager;
#[cfg(test)]
use crate::cgroups::mock::Manager as FsManager;
use crate::cgroups::{DevicesCgroupInfo, Manager};
use crate::log_child;
use crate::process::Process;
use crate::process::ProcessOperations;
#[cfg(feature = "seccomp")]
use crate::seccomp;
use crate::selinux;
use crate::specconv::CreateOpts;
use crate::{mount, validator};

use nix::errno::Errno;
use nix::fcntl::{self, OFlag};
use nix::fcntl::{FcntlArg, FdFlag};
use nix::mount::MntFlags;
use nix::pty;
use nix::sched::{self, CloneFlags};
use nix::sys::signal::{self, Signal};
use nix::sys::stat::{self, Mode};
use nix::unistd::{self, fork, ForkResult, Gid, Pid, Uid, User};
use std::os::unix::fs::MetadataExt;
use std::os::unix::io::AsRawFd;

use std::collections::HashMap;
use std::os::unix::io::FromRawFd;
use std::str::FromStr;
use std::sync::{Arc, RwLock};

use slog::{info, o, Logger};

use crate::pipestream::PipeStream;
use crate::sync::{read_sync, write_count, write_sync, SYNC_DATA, SYNC_FAILED, SYNC_SUCCESS};
use crate::sync_with_async::{read_async, write_async};
use async_trait::async_trait;
use rlimit::{setrlimit, Resource, Rlim};
use tokio::io::AsyncBufReadExt;
use tokio::sync::Mutex;

use kata_sys_util::validate::valid_env;

pub const EXEC_FIFO_FILENAME: &str = "exec.fifo";

const INIT: &str = "INIT";
const NO_PIVOT: &str = "NO_PIVOT";
const CRFD_FD: &str = "CRFD_FD";
const CWFD_FD: &str = "CWFD_FD";
const CLOG_FD: &str = "CLOG_FD";
const FIFO_FD: &str = "FIFO_FD";
const HOME_ENV_KEY: &str = "HOME";
const PIDNS_FD: &str = "PIDNS_FD";

#[derive(Debug)]
pub struct ContainerStatus {
    cur_status: ContainerState,
}

impl ContainerStatus {
    pub fn new() -> Self {
        ContainerStatus {
            cur_status: ContainerState::Created,
        }
    }

    fn status(&self) -> ContainerState {
        self.cur_status
    }

    fn transition(&mut self, to: ContainerState) {
        self.cur_status = to;
    }
}

impl Default for ContainerStatus {
    fn default() -> Self {
        Self::new()
    }
}

// We might want to change this to thiserror in the future
const MissingLinux: &str = "no linux config";
const InvalidNamespace: &str = "invalid namespace type";

pub type Config = CreateOpts;

lazy_static! {
    // This locker ensures the child exit signal will be received by the right receiver.
    pub static ref WAIT_PID_LOCKER: Arc<Mutex<bool>> = Arc::new(Mutex::new(false));

    pub static ref NAMESPACES: HashMap<&'static str, CloneFlags> = {
        let mut m = HashMap::new();
        m.insert("user", CloneFlags::CLONE_NEWUSER);
        m.insert("ipc", CloneFlags::CLONE_NEWIPC);
        m.insert("pid", CloneFlags::CLONE_NEWPID);
        m.insert("net", CloneFlags::CLONE_NEWNET);
        m.insert("mnt", CloneFlags::CLONE_NEWNS);
        m.insert("uts", CloneFlags::CLONE_NEWUTS);
        m.insert("cgroup", CloneFlags::CLONE_NEWCGROUP);
        m
    };

    // type to name hashmap, better to be in NAMESPACES
    pub static ref TYPETONAME: HashMap<oci::LinuxNamespaceType, &'static str> = {
        let mut m = HashMap::new();
        m.insert(oci::LinuxNamespaceType::Ipc, "ipc");
        m.insert(oci::LinuxNamespaceType::User, "user");
        m.insert(oci::LinuxNamespaceType::Pid, "pid");
        m.insert(oci::LinuxNamespaceType::Network, "net");
        m.insert(oci::LinuxNamespaceType::Mount, "mnt");
        m.insert(oci::LinuxNamespaceType::Cgroup, "cgroup");
        m.insert(oci::LinuxNamespaceType::Uts, "uts");
        m
    };

    pub static ref DEFAULT_DEVICES: Vec<LinuxDevice> = {
        vec![
            oci::LinuxDeviceBuilder::default()
                .path(PathBuf::from("/dev/null"))
                .typ(oci::LinuxDeviceType::C)
                .major(1)
                .minor(3)
                .file_mode(0o666_u32)
                .uid(0xffffffff_u32)
                .gid(0xffffffff_u32)
                .build()
                .unwrap(),
            oci::LinuxDeviceBuilder::default()
                .path(PathBuf::from("/dev/zero"))
                .typ(oci::LinuxDeviceType::C)
                .major(1)
                .minor(5)
                .file_mode(0o666_u32)
                .uid(0xffffffff_u32)
                .gid(0xffffffff_u32)
                .build()
                .unwrap(),
            oci::LinuxDeviceBuilder::default()
                .path(PathBuf::from("/dev/full"))
                .typ(oci::LinuxDeviceType::C)
                .major(1)
                .minor(7)
                .file_mode(0o666_u32)
                .uid(0xffffffff_u32)
                .gid(0xffffffff_u32)
                .build()
                .unwrap(),
            oci::LinuxDeviceBuilder::default()
                .path(PathBuf::from("/dev/tty"))
                .typ(oci::LinuxDeviceType::C)
                .major(5)
                .minor(0)
                .file_mode(0o666_u32)
                .uid(0xffffffff_u32)
                .gid(0xffffffff_u32)
                .build()
                .unwrap(),
            oci::LinuxDeviceBuilder::default()
                .path(PathBuf::from("/dev/urandom"))
                .typ(oci::LinuxDeviceType::C)
                .major(1)
                .minor(9)
                .file_mode(0o666_u32)
                .uid(0xffffffff_u32)
                .gid(0xffffffff_u32)
                .build()
                .unwrap(),
            oci::LinuxDeviceBuilder::default()
                .path(PathBuf::from("/dev/random"))
                .typ(oci::LinuxDeviceType::C)
                .major(1)
                .minor(8)
                .file_mode(0o666_u32)
                .uid(0xffffffff_u32)
                .gid(0xffffffff_u32)
                .build()
                .unwrap(),
        ]
    };
}

#[async_trait]
pub trait BaseContainer {
    fn status(&self) -> ContainerState;
    fn get_process(&mut self, eid: &str) -> Result<&mut Process>;
    fn set(&mut self, config: LinuxResources) -> Result<()>;
    async fn start(&mut self, p: Process) -> Result<()>;
    async fn run(&mut self, p: Process) -> Result<()>;
    async fn destroy(&mut self) -> Result<()>;
    async fn exec(&mut self) -> Result<()>;
}

// LinuxContainer protected by Mutex
// Arc<Mutex<Innercontainer>> or just Mutex<InnerContainer>?
// Or use Mutex<xx> as a member of struct, like C?
// a lot of String in the struct might be &str
#[derive(Debug)]
pub struct LinuxContainer {
    pub id: String,
    pub root: String,
    pub config: Config,
    pub cgroup_manager: Arc<dyn Manager + Send + Sync>,
    pub init_process_pid: pid_t,
    pub processes: HashMap<String, Process>,
    pub status: ContainerStatus,
    pub logger: Logger,
    // The map only owns the files; later execs use the corresponding agent proc-fd
    // paths stored in the OCI spec. Keeping the files open makes those paths continue
    // to resolve to the original namespaces.
    pinned_namespace_fds: HashMap<oci::LinuxNamespaceType, fs::File>,
}

pub trait Container: BaseContainer {
    fn pause(&mut self) -> Result<()>;
    fn resume(&mut self) -> Result<()>;
}

impl Container for LinuxContainer {
    fn pause(&mut self) -> Result<()> {
        let status = self.status();
        if status != ContainerState::Running && status != ContainerState::Created {
            return Err(anyhow!(
                "failed to pause container: current status is: {:?}",
                status
            ));
        }

        self.cgroup_manager.as_ref().freeze(FreezerState::Frozen)?;

        self.status.transition(ContainerState::Paused);

        Ok(())
    }

    fn resume(&mut self) -> Result<()> {
        let status = self.status();
        if status != ContainerState::Paused {
            return Err(anyhow!("container status is: {:?}, not paused", status));
        }

        self.cgroup_manager.as_ref().freeze(FreezerState::Thawed)?;

        self.status.transition(ContainerState::Running);

        Ok(())
    }
}

pub fn init_child() {
    let cwfd = std::env::var(CWFD_FD).unwrap().parse::<i32>().unwrap();
    let cfd_log = std::env::var(CLOG_FD).unwrap().parse::<i32>().unwrap();

    match do_init_child(cwfd) {
        Ok(_) => log_child!(cfd_log, "temporary parent process exit successfully"),
        Err(e) => {
            log_child!(cfd_log, "temporary parent process exit:child exit: {:?}", e);
            let _ = write_sync(cwfd, SYNC_FAILED, format!("{e:?}").as_str());
        }
    }
}

fn do_init_child(cwfd: RawFd) -> Result<()> {
    lazy_static::initialize(&NAMESPACES);
    lazy_static::initialize(&DEFAULT_DEVICES);

    let init = std::env::var(INIT)?.eq(format!("{}", true).as_str());

    let no_pivot = std::env::var(NO_PIVOT)?.eq(format!("{}", true).as_str());
    let crfd = std::env::var(CRFD_FD)?.parse::<i32>().unwrap();
    let cfd_log = std::env::var(CLOG_FD)?.parse::<i32>().unwrap();

    // get the pidns fd from parent, if parent had passed the pidns fd,
    // then get it and join in this pidns; otherwise, create a new pidns
    // by unshare from the parent pidns.
    match std::env::var(PIDNS_FD) {
        Ok(fd) => {
            let pidns_fd =
                unsafe { OwnedFd::from_raw_fd(fd.parse::<i32>().context("get parent pidns fd")?) };
            sched::setns(&pidns_fd, CloneFlags::CLONE_NEWPID).context("failed to join pidns")?;
            // close is automatic on drop
        }
        Err(_e) => {
            sched::unshare(CloneFlags::CLONE_NEWPID)?;
        }
    }

    match unsafe { fork() } {
        Ok(ForkResult::Parent { child, .. }) => {
            log_child!(
                cfd_log,
                "Continuing execution in temporary process, new child has pid: {:?}",
                child
            );
            let _ = write_sync(cwfd, SYNC_DATA, format!("{}", pid_t::from(child)).as_str());
            // parent return
            return Ok(());
        }
        Ok(ForkResult::Child) => (),
        Err(e) => {
            return Err(anyhow!(format!(
                "failed to fork temporary process: {:?}",
                e
            )));
        }
    }
    log_child!(cfd_log, "child process start run");
    let buf = read_sync(crfd)?;
    let spec_str = std::str::from_utf8(&buf)?;
    let spec: oci::Spec = serde_json::from_str(spec_str)?;
    log_child!(cfd_log, "notify parent to send oci process");
    write_sync(cwfd, SYNC_SUCCESS, "")?;

    let buf = read_sync(crfd)?;
    let process_str = std::str::from_utf8(&buf)?;
    let oci_process: oci::Process = serde_json::from_str(process_str)?;
    write_sync(cwfd, SYNC_SUCCESS, "")?;

    let p = spec
        .process()
        .as_ref()
        .ok_or_else(|| anyhow!("didn't find process in Spec"))?;

    let linux = spec.linux().as_ref().ok_or_else(|| anyhow!(MissingLinux))?;

    // get namespace vector to join/new
    let nses = get_namespaces(linux);

    let mut userns = false;
    let mut to_new = CloneFlags::empty();
    let mut to_join: Vec<(CloneFlags, OwnedFd)> = Vec::new();

    for ns in &nses {
        let ns_type = ns.typ().to_string();
        let s = NAMESPACES.get(&ns_type.as_str());
        if s.is_none() {
            return Err(anyhow!(InvalidNamespace));
        }
        let s = s.unwrap();

        if ns.path().as_ref().is_none_or(|p| p.as_os_str().is_empty()) {
            // skip the pidns since it has been done in parent process.
            if *s != CloneFlags::CLONE_NEWPID {
                to_new.set(*s, true);
            }
        } else {
            let fd = fcntl::open(ns.path().as_ref().unwrap(), OFlag::O_CLOEXEC, Mode::empty())
                .inspect_err(|e| {
                    log_child!(
                        cfd_log,
                        "cannot open type: {} path: {}",
                        &ns.typ().to_string(),
                        ns.path().as_ref().unwrap().display()
                    );
                    log_child!(cfd_log, "error is : {:?}", e)
                })?;

            if *s != CloneFlags::CLONE_NEWPID {
                to_join.push((*s, fd));
            }
        }
    }

    if to_new.contains(CloneFlags::CLONE_NEWUSER) {
        userns = true;
    }

    if p.oom_score_adj().is_some() {
        log_child!(cfd_log, "write oom score {}", p.oom_score_adj().unwrap());
        fs::write(
            "/proc/self/oom_score_adj",
            p.oom_score_adj().unwrap().to_string().as_bytes(),
        )?;
    }

    // set rlimit
    let default_rlimits = Vec::new();
    let process_rlimits = p.rlimits().as_ref().unwrap_or(&default_rlimits);
    for rl in process_rlimits.iter() {
        log_child!(cfd_log, "set resource limit: {:?}", rl);
        setrlimit(
            Resource::from_str(&rl.typ().to_string())?,
            Rlim::from_raw(rl.soft()),
            Rlim::from_raw(rl.hard()),
        )?;
    }

    //
    // Make the process non-dumpable, to avoid various race conditions that
    // could cause processes in namespaces we're joining to access host
    // resources (or potentially execute code).
    //
    // However, if the number of namespaces we are joining is 0, we are not
    // going to be switching to a different security context. Thus setting
    // ourselves to be non-dumpable only breaks things (like rootless
    // containers), which is the recommendation from the kernel folks.
    //
    // Ref: https://github.com/opencontainers/runc/commit/50a19c6ff828c58e5dab13830bd3dacde268afe5
    //
    if !nses.is_empty() {
        capctl::prctl::set_dumpable(false)
            .map_err(|e| anyhow!(e).context("set process non-dumpable failed"))?;
    }

    if userns {
        log_child!(cfd_log, "enter new user namespace");
        sched::unshare(CloneFlags::CLONE_NEWUSER)?;
    }

    log_child!(cfd_log, "notify parent unshare user ns completed");
    // notify parent unshare user ns completed.
    write_sync(cwfd, SYNC_SUCCESS, "")?;
    // wait parent to setup user id mapping.
    log_child!(cfd_log, "wait parent to setup user id mapping");
    read_sync(crfd)?;

    if userns {
        log_child!(cfd_log, "setup user id");
        setid(Uid::from_raw(0), Gid::from_raw(0))?;
    }

    let mut mount_fd: Option<OwnedFd> = None;
    let mut bind_device = false;
    for (s, fd) in to_join {
        if s == CloneFlags::CLONE_NEWNS {
            mount_fd = Some(fd);
            continue;
        }

        log_child!(cfd_log, "join namespace {:?}", s);
        sched::setns(&fd, s).or_else(|e| {
            if s == CloneFlags::CLONE_NEWUSER {
                if e != Errno::EINVAL {
                    let _ = write_sync(cwfd, SYNC_FAILED, format!("{e:?}").as_str());
                    return Err(e);
                }

                Ok(())
            } else {
                let _ = write_sync(cwfd, SYNC_FAILED, format!("{e:?}").as_str());
                Err(e)
            }
        })?;

        unistd::close(fd)?;

        if s == CloneFlags::CLONE_NEWUSER {
            setid(Uid::from_raw(0), Gid::from_raw(0))?;
            bind_device = true;
        }
    }

    let selinux_enabled = selinux::is_enabled()?;

    sched::unshare(to_new & !CloneFlags::CLONE_NEWUSER)?;

    sched::unshare(CloneFlags::CLONE_NEWCGROUP)?;

    if userns {
        bind_device = true;
    }

    if to_new.contains(CloneFlags::CLONE_NEWUTS) {
        unistd::sethostname(
            spec.hostname()
                .as_ref()
                .map_or("".to_string(), |x| x.clone()),
        )?;
    }

    let rootfs = spec.root().as_ref().unwrap().path().display().to_string();

    log_child!(cfd_log, "setup rootfs {}", &rootfs);
    let root = fs::canonicalize(&rootfs)?;
    let rootfs = root.to_str().unwrap();

    if to_new.contains(CloneFlags::CLONE_NEWNS) {
        mount::init_rootfs(cfd_log, &spec, bind_device)?;
    }

    if let Some(mount_fd) = mount_fd {
        sched::setns(&mount_fd, CloneFlags::CLONE_NEWNS)?;
        // mount_fd will be automatically closed when dropped
    }

    if to_new.contains(CloneFlags::CLONE_NEWNS) {
        // unistd::chroot(rootfs)?;
        if no_pivot {
            mount::ms_move_root(rootfs)?;
        } else {
            // pivot root
            mount::pivot_rootfs(rootfs)?;
        }

        // setup sysctl
        set_sysctls(&linux.sysctl().clone().unwrap_or_default())?;
        unistd::chdir("/")?;
    }

    if to_new.contains(CloneFlags::CLONE_NEWNS) {
        mount::finish_rootfs(cfd_log, &spec, &oci_process)?;
    }

    if !oci_process.cwd().as_os_str().is_empty() {
        unistd::chdir(oci_process.cwd().display().to_string().as_str())?;
    }
    verify_cwd()?;
    // Create new session for init/exec process rather than inheriting parent's session
    // This ensures the init/exec process owns independent session ID, which aligns with runc behavior.
    unistd::setsid().context("create a new session")?;

    let guser = &oci_process.user();

    let uid = Uid::from_raw(guser.uid());
    let gid = Gid::from_raw(guser.gid());

    // only change stdio devices owner when user
    // isn't root.
    if !uid.is_root() {
        set_stdio_permissions(uid)?;
    }

    setid(uid, gid)?;

    if let Some(additional_gids) = guser.additional_gids() {
        let gids: Vec<Gid> = additional_gids
            .iter()
            .map(|gid| Gid::from_raw(*gid))
            .collect();

        unistd::setgroups(&gids).inspect_err(|e| {
            let _ = write_sync(
                cwfd,
                SYNC_FAILED,
                format!("setgroups failed: {e:?}").as_str(),
            );
        })?;
    }

    // NoNewPrivileges
    if oci_process.no_new_privileges().unwrap_or_default() {
        capctl::prctl::set_no_new_privs().map_err(|_| anyhow!("cannot set no new privileges"))?;
    }

    // Set SELinux label
    if !oci_process
        .selinux_label()
        .clone()
        .unwrap_or_default()
        .is_empty()
    {
        if !selinux_enabled {
            return Err(anyhow!(
                "SELinux label for the process is provided but SELinux is not enabled on the running kernel"
            ));
        }

        log_child!(cfd_log, "Set SELinux label to the container process");
        let default_label = String::new();
        selinux::set_exec_label(
            oci_process
                .selinux_label()
                .as_ref()
                .unwrap_or(&default_label),
        )?;
    }

    // Log unknown seccomp system calls in advance before the log file descriptor closes.
    #[cfg(feature = "seccomp")]
    if let Some(ref scmp) = linux.seccomp() {
        if let Some(syscalls) = seccomp::get_unknown_syscalls(scmp) {
            log_child!(cfd_log, "unknown seccomp system calls: {:?}", syscalls);
        }
    }

    // Restrict the bounding set before installing the workload seccomp filter.
    // A filter can reject selected PR_CAPBSET_READ operations and make kernel
    // capability discovery appear incomplete.
    if let Some(caps) = oci_process.capabilities().as_ref() {
        capabilities::restrict_bounding_set(cfd_log, caps)?;
    }

    // Without NoNewPrivileges, install seccomp while the calling thread still
    // has effective CAP_SYS_ADMIN. The bounding set is already restricted.
    #[cfg(feature = "seccomp")]
    if !oci_process.no_new_privileges().unwrap_or_default() {
        if let Some(ref scmp) = linux.seccomp() {
            seccomp::init_seccomp(scmp)?;
        }
    }

    // Drop remaining capabilities
    if oci_process.capabilities().is_some() {
        let c = oci_process.capabilities().as_ref().unwrap();
        capabilities::drop_privileges(cfd_log, c)?;
    }

    let default_vec = Vec::new();
    let args = oci_process.args().as_ref().unwrap_or(&default_vec).to_vec();
    let env = oci_process.env().as_ref().unwrap_or(&default_vec).to_vec();

    let mut fifofd = -1;
    if init {
        fifofd = std::env::var(FIFO_FD)?.parse::<i32>().unwrap();
    }

    // cleanup the env inherited from parent
    for (key, _) in env::vars() {
        env::remove_var(key);
    }

    // setup the envs
    for e in env.iter() {
        match valid_env(e) {
            Some((key, value)) => env::set_var(key, value),
            None => log_child!(cfd_log, "invalid env key-value: {:?}", e),
        }
    }

    if env::var_os(HOME_ENV_KEY).is_none() {
        // try to set "HOME" env by uid
        if let Ok(Some(user)) = User::from_uid(Uid::from_raw(guser.uid())) {
            if let Ok(user_home_dir) = user.dir.into_os_string().into_string() {
                env::set_var(HOME_ENV_KEY, user_home_dir);
            }
        }
        // set default home dir as "/" if "HOME" env is still empty
        if env::var_os(HOME_ENV_KEY).is_none() {
            env::set_var(HOME_ENV_KEY, String::from("/"));
        }
    }

    let exec_file = Path::new(&args[0]);
    log_child!(cfd_log, "process command: {:?}", &args);
    if !exec_file.exists() {
        find_file(exec_file).ok_or_else(|| anyhow!("the file {} was not found", &args[0]))?;
    }

    // notify parent that the child's ready to start
    write_sync(cwfd, SYNC_SUCCESS, "")?;
    log_child!(cfd_log, "ready to run exec");
    let _ = unistd::close(cfd_log);
    let _ = unistd::close(crfd);
    let _ = unistd::close(cwfd);

    if oci_process.terminal().unwrap_or_default() {
        unsafe { libc::ioctl(0, libc::TIOCSCTTY) };
    }

    if init {
        let fd = fcntl::open(
            format!("/proc/self/fd/{fifofd}").as_str(),
            OFlag::O_RDONLY | OFlag::O_CLOEXEC,
            Mode::from_bits_truncate(0),
        )?;
        unistd::close(fifofd)?;
        let buf: &mut [u8] = &mut [0];
        unistd::read(fd, buf)?;
    }

    // With NoNewPrivileges, we should set seccomp as close to
    // do_exec as possible in order to reduce the amount of
    // system calls in the seccomp profiles.
    #[cfg(feature = "seccomp")]
    if oci_process.no_new_privileges().unwrap_or_default() {
        if let Some(ref scmp) = linux.seccomp() {
            seccomp::init_seccomp(scmp)?;
        }
    }

    do_exec(&args);
}

// Verify that chdir did not follow a procfs magic link outside the container
// mount namespace. libc reports ENOENT when cwd is unreachable from root.
fn verify_cwd() -> Result<()> {
    match unistd::getcwd() {
        Err(Errno::ENOENT) => Err(anyhow!(
            "current working directory is outside the container mount namespace root"
        )),
        Err(e) => Err(anyhow!("failed to verify current working directory: {e}")),
        Ok(_) => Ok(()),
    }
}

// set_stdio_permissions fixes the permissions of PID 1's STDIO
// within the container to the specified user.
// The ownership needs to match because it is created outside of
// the container and needs to be localized.
fn set_stdio_permissions(uid: Uid) -> Result<()> {
    let meta = fs::metadata("/dev/null")?;
    let fds = [
        std::io::stdin().as_raw_fd(),
        std::io::stdout().as_raw_fd(),
        std::io::stderr().as_raw_fd(),
    ];

    for fd in &fds {
        let borrowed_fd = unsafe { BorrowedFd::borrow_raw(*fd) };
        let stat = stat::fstat(borrowed_fd)?;
        // Skip chown of /dev/null if it was used as one of the STDIO fds.
        if stat.st_rdev == meta.rdev() {
            continue;
        }

        // We only change the uid owner (as it is possible for the mount to
        // prefer a different gid, and there's no reason for us to change it).
        // The reason why we don't just leave the default uid=X mount setup is
        // that users expect to be able to actually use their console. Without
        // this code, you couldn't effectively run as a non-root user inside a
        // container and also have a console set up.
        unistd::fchown(borrowed_fd, Some(uid), None)
            .with_context(|| "set stdio permissions failed")?;
    }

    Ok(())
}

#[async_trait]
impl BaseContainer for LinuxContainer {
    fn status(&self) -> ContainerState {
        self.status.status()
    }

    fn get_process(&mut self, eid: &str) -> Result<&mut Process> {
        self.processes
            .get_mut(eid)
            .ok_or_else(|| anyhow!("invalid eid {}", eid))
    }

    fn set(&mut self, r: LinuxResources) -> Result<()> {
        self.cgroup_manager.as_ref().set(&r)?;

        if let Some(linux) = self.config.spec.as_mut().unwrap().linux_mut() {
            linux.set_resources(Some(r));
        }

        Ok(())
    }

    async fn start(&mut self, mut p: Process) -> Result<()> {
        let logger = self.logger.new(o!("eid" => p.exec_id.clone()));

        // Check if exec_id is already in use to prevent collisions
        if self.processes.contains_key(p.exec_id.as_str()) {
            return Err(anyhow!("exec_id '{}' already exists", p.exec_id));
        }

        let tty = p.tty;
        let fifo_file = format!("{}/{}", &self.root, EXEC_FIFO_FILENAME);
        info!(logger, "enter container.start!");
        let mut fifofd: RawFd = -1;
        if p.init {
            if stat::stat(fifo_file.as_str()).is_ok() {
                return Err(anyhow!("exec fifo exists"));
            }
            unistd::mkfifo(fifo_file.as_str(), Mode::from_bits(0o644).unwrap())?;

            let fd = fcntl::open(
                fifo_file.as_str(),
                OFlag::O_PATH,
                Mode::from_bits(0).unwrap(),
            )?;
            fifofd = fd.into_raw_fd();
        }
        info!(logger, "exec fifo opened!");

        if self.config.spec.is_none() {
            return Err(anyhow!("no spec"));
        }

        let spec = self.config.spec.as_ref().unwrap();
        if spec.linux().is_none() {
            return Err(anyhow!("no linux config"));
        }
        let linux = spec.linux().as_ref().unwrap();

        if p.oci.capabilities().is_none() {
            // No capabilities, inherit from container process
            let process = spec
                .process()
                .as_ref()
                .ok_or_else(|| anyhow!("no process config"))?;
            p.oci.set_capabilities(Some(
                process
                    .capabilities()
                    .clone()
                    .ok_or_else(|| anyhow!("missing process capabilities"))?,
            ));
        }

        let (pfd_log, cfd_log) = unistd::pipe().context("failed to create pipe")?;

        let _ = fcntl::fcntl(&pfd_log, FcntlArg::F_SETFD(FdFlag::FD_CLOEXEC))
            .map_err(|e| warn!(logger, "fcntl pfd log FD_CLOEXEC {:?}", e));

        let child_logger = logger.new(o!("action" => "child process log"));
        let log_stream = PipeStream::new(pfd_log.into_raw_fd())?;
        let log_handler = setup_child_logger(log_stream, child_logger);

        let (prfd, cwfd) = unistd::pipe().context("failed to create pipe")?;
        let (crfd, pwfd) = unistd::pipe().context("failed to create pipe")?;

        let _ = fcntl::fcntl(&prfd, FcntlArg::F_SETFD(FdFlag::FD_CLOEXEC))
            .map_err(|e| warn!(logger, "fcntl prfd FD_CLOEXEC {:?}", e));

        let _ = fcntl::fcntl(&pwfd, FcntlArg::F_SETFD(FdFlag::FD_CLOEXEC))
            .map_err(|e| warn!(logger, "fcntl pwfd FD_COLEXEC {:?}", e));

        let mut pipe_r = PipeStream::new(prfd.into_raw_fd())?;
        let mut pipe_w = PipeStream::new(pwfd.into_raw_fd())?;

        let child_stdin: std::process::Stdio;
        let child_stdout: std::process::Stdio;
        let child_stderr: std::process::Stdio;

        if tty {
            let pseudo = pty::openpty(None, None)?;
            let _ = fcntl::fcntl(&pseudo.master, FcntlArg::F_SETFD(FdFlag::FD_CLOEXEC))
                .map_err(|e| warn!(logger, "fnctl pseudo.master {:?}", e));
            let _ = fcntl::fcntl(&pseudo.slave, FcntlArg::F_SETFD(FdFlag::FD_CLOEXEC))
                .map_err(|e| warn!(logger, "fcntl pseudo.slave {:?}", e));

            // Transfer ownership of master to raw fd for multiple uses
            let master_raw = pseudo.master.into_raw_fd();
            p.term_master = Some(master_raw);

            let slave_raw = pseudo.slave.into_raw_fd();
            child_stdin = unsafe { std::process::Stdio::from_raw_fd(slave_raw) };
            // Create temporary OwnedFd for dup operations, then forget it since stdin owns the fd
            let slave_fd = unsafe { OwnedFd::from_raw_fd(slave_raw) };
            child_stdout =
                unsafe { std::process::Stdio::from_raw_fd(unistd::dup(&slave_fd)?.into_raw_fd()) };
            child_stderr =
                unsafe { std::process::Stdio::from_raw_fd(unistd::dup(&slave_fd)?.into_raw_fd()) };
            std::mem::forget(slave_fd); // Don't close - stdin owns it
        } else {
            // not using a terminal
            let stdin = p.stdin.unwrap();
            let stdout = p.stdout.unwrap();
            let stderr = p.stderr.unwrap();
            child_stdin = unsafe { std::process::Stdio::from_raw_fd(stdin) };
            child_stdout = unsafe { std::process::Stdio::from_raw_fd(stdout) };
            child_stderr = unsafe { std::process::Stdio::from_raw_fd(stderr) };
        }

        let pidns = get_pid_namespace(&self.logger, linux)?;
        defer!(if let Some(fd) = pidns {
            let _ = unistd::close(fd);
        });

        let exec_path = std::env::current_exe()?;
        let mut child = std::process::Command::new(exec_path);

        let mut child = child
            .arg("init")
            .stdin(child_stdin)
            .stdout(child_stdout)
            .stderr(child_stderr)
            .env(INIT, format!("{}", p.init))
            .env(NO_PIVOT, format!("{}", self.config.no_pivot_root))
            .env(CRFD_FD, format!("{}", crfd.as_fd().as_raw_fd()))
            .env(CWFD_FD, format!("{}", cwfd.as_fd().as_raw_fd()))
            .env(CLOG_FD, format!("{}", cfd_log.as_fd().as_raw_fd()));

        if p.init {
            child = child.env(FIFO_FD, format!("{fifofd}"));
        }

        if let Some(fd) = pidns {
            child = child.env(PIDNS_FD, format!("{fd}"));
        }

        child.spawn()?;

        // OwnedFd will be automatically closed when dropped
        drop(crfd);
        drop(cwfd);
        drop(cfd_log);

        // get container process's pid
        let pid_buf = read_async(&mut pipe_r).await?;
        let pid_str = std::str::from_utf8(&pid_buf).context("get pid string")?;
        let pid = match pid_str.parse::<i32>() {
            Ok(i) => i,
            Err(e) => {
                return Err(anyhow!(format!(
                    "failed to get container process's pid: {:?}",
                    e
                )));
            }
        };

        p.pid = pid;

        if p.init {
            self.init_process_pid = p.pid;
        }

        if p.init {
            let _ = unistd::close(fifofd).map_err(|e| warn!(logger, "close fifofd {:?}", e));
        }

        info!(logger, "child pid: {}", p.pid);

        join_namespaces(
            &logger,
            spec,
            &p,
            self.cgroup_manager.as_ref(),
            &mut pipe_w,
            &mut pipe_r,
        )
        .await
        .map_err(|e| {
            error!(logger, "create container process error {:?}", e);
            // kill the child process.
            let _ = signal::kill(Pid::from_raw(p.pid), Some(Signal::SIGKILL))
                .map_err(|e| warn!(logger, "signal::kill joining namespaces {:?}", e));

            e
        })?;

        info!(logger, "entered namespaces!");

        if p.init {
            let spec = self.config.spec.as_mut().unwrap();
            if let Err(e) =
                update_namespaces(&self.logger, spec, p.pid, &mut self.pinned_namespace_fds)
            {
                // Pinning failed before the init process was added to self.processes.
                // Kill it so an untracked child is not left blocked on the exec FIFO.
                let _ = signal::kill(Pid::from_raw(p.pid), Some(Signal::SIGKILL)).map_err(
                    |kill_error| warn!(logger, "failed to kill process: {:?}", kill_error),
                );
                return Err(e);
            }
        }
        self.processes.insert(p.exec_id.clone(), p);

        info!(logger, "wait on child log handler");
        let _ = log_handler
            .await
            .map_err(|e| warn!(logger, "joining log handler {:?}", e));
        info!(logger, "create process completed");
        Ok(())
    }

    async fn run(&mut self, p: Process) -> Result<()> {
        let init = p.init;
        self.start(p).await?;

        if init {
            self.exec().await?;
            self.status.transition(ContainerState::Running);
        }

        Ok(())
    }

    async fn destroy(&mut self) -> Result<()> {
        let spec = self.config.spec.as_ref().unwrap();
        for process in self.processes.values() {
            match signal::kill(process.pid(), Some(Signal::SIGKILL)) {
                Err(Errno::ESRCH) => {
                    info!(
                        self.logger,
                        "kill encounters ESRCH, pid: {}, container: {}",
                        process.pid(),
                        self.id.clone()
                    );
                    continue;
                }
                Err(err) => return Err(anyhow!(err)),
                Ok(_) => continue,
            }
        }

        self.status.transition(ContainerState::Stopped);

        // Kill all of the processes created in this container to prevent
        // the leak of some daemon process when this container shared pidns
        // with the sandbox.
        let cgm = self.cgroup_manager.as_ref();
        let pids = cgm.get_pids().context("get cgroup pids")?;
        info!(
            self.logger,
            "destroy: container {} cgroup has {} processes: {:?}",
            self.id,
            pids.len(),
            pids
        );
        for i in &pids {
            info!(
                self.logger,
                "destroy: killing process {} in container {}", i, self.id
            );
            if let Err(e) = signal::kill(Pid::from_raw(*i), Signal::SIGKILL) {
                warn!(self.logger, "kill the process {} error: {:?}", i, e);
            }
        }

        info!(
            self.logger,
            "destroy: destroying cgroup for container {}", self.id
        );
        cgm.destroy().context("destroy cgroups")?;

        // Now umount and remove the container's root directory.
        // This is done after process cleanup to ensure processes are killed
        // even if filesystem cleanup fails (e.g., due to read-only mounts).
        mount::umount2(
            spec.root()
                .as_ref()
                .unwrap()
                .path()
                .display()
                .to_string()
                .as_str(),
            MntFlags::MNT_DETACH,
        )
        .or_else(|e| {
            if e.ne(&nix::Error::EINVAL) {
                return Err(anyhow!(e));
            }
            warn!(self.logger, "rootfs not mounted");
            Ok(())
        })?;
        fs::remove_dir_all(&self.root)?;

        Ok(())
    }

    async fn exec(&mut self) -> Result<()> {
        let fifo = format!("{}/{}", &self.root, EXEC_FIFO_FILENAME);
        let fd = fcntl::open(fifo.as_str(), OFlag::O_WRONLY, Mode::from_bits_truncate(0))?;
        let data: &[u8] = &[0];
        unistd::write(&fd, data)?;
        info!(self.logger, "container started");
        self.status.transition(ContainerState::Running);

        unistd::close(fd)?;

        Ok(())
    }
}

use std::env;

fn find_file<P>(exe_name: P) -> Option<PathBuf>
where
    P: AsRef<Path>,
{
    env::var_os("PATH").and_then(|paths| {
        env::split_paths(&paths)
            .filter_map(|dir| {
                let full_path = dir.join(&exe_name);
                if full_path.is_file() {
                    Some(full_path)
                } else {
                    None
                }
            })
            .next()
    })
}

fn do_exec(args: &[String]) -> ! {
    let path = &args[0];
    let p = CString::new(path.to_string()).unwrap();
    let sa: Vec<CString> = args
        .iter()
        .map(|s| CString::new(s.to_string()).unwrap_or_default())
        .collect();

    let _ = unistd::execvp(p.as_c_str(), &sa).map_err(|e| match e {
        nix::Error::UnknownErrno => std::process::exit(-2),
        _ => std::process::exit(e as i32),
    });

    unreachable!()
}

pub fn update_namespaces(
    logger: &Logger,
    spec: &mut Spec,
    init_pid: RawFd,
    pinned_namespace_fds: &mut HashMap<oci::LinuxNamespaceType, fs::File>,
) -> Result<()> {
    info!(logger, "updating namespaces");
    let linux = spec
        .linux_mut()
        .as_mut()
        .ok_or_else(|| anyhow!("Spec didn't contain linux field"))?;

    if let Some(namespaces) = linux.namespaces_mut().as_mut() {
        for namespace in namespaces.iter_mut() {
            if TYPETONAME.contains_key(&namespace.typ()) {
                let ns_path = format!(
                    "/proc/{}/ns/{}",
                    init_pid,
                    TYPETONAME.get(&namespace.typ()).unwrap()
                );

                if namespace
                    .path()
                    .as_ref()
                    .is_none_or(|p| p.as_os_str().is_empty())
                {
                    // This path disappears when the init process exits. If its numeric PID is later
                    // reused, the same path can identify an unrelated process's namespace. Open it
                    // now to pin the original namespace independently of the init process's lifetime.
                    let fd = fcntl::open(
                        ns_path.as_str(),
                        OFlag::O_RDONLY | OFlag::O_CLOEXEC,
                        Mode::empty(),
                    )
                    .with_context(|| format!("failed to pin namespace {}", ns_path))?;
                    let file = fs::File::from(fd);

                    // Later namespace setup runs in a separately exec'd helper. This O_CLOEXEC FD
                    // remains owned by the agent, so address it through the agent's proc directory;
                    // `/proc/self/fd` in the helper would refer to the helper's descriptor table.
                    let agent_pid = std::process::id();
                    let namespace_fd = file.as_raw_fd();
                    let pinned_path = PathBuf::from(format!("/proc/{agent_pid}/fd/{namespace_fd}"));

                    namespace.set_path(Some(pinned_path));
                    pinned_namespace_fds.insert(namespace.typ(), file);
                }
            }
        }
    }

    Ok(())
}

fn get_pid_namespace(logger: &Logger, linux: &Linux) -> Result<Option<i32>> {
    let linux_namespaces = linux.namespaces().clone().unwrap_or_default();
    for ns in &linux_namespaces {
        if &ns.typ().to_string() == "pid" {
            let fd = match ns.path() {
                None => return Ok(None),
                Some(ns_path) => fcntl::open(
                    ns_path.display().to_string().as_str(),
                    OFlag::O_RDONLY,
                    Mode::empty(),
                )
                .inspect_err(|e| {
                    error!(
                        logger,
                        "cannot open type: {} path: {}",
                        &ns.typ().to_string(),
                        ns_path.display()
                    );
                    error!(logger, "error is : {:?}", e)
                })?,
            };

            return Ok(Some(fd.into_raw_fd()));
        }
    }

    Err(anyhow!("cannot find the pid ns"))
}

fn is_userns_enabled(linux: &Linux) -> bool {
    linux
        .namespaces()
        .clone()
        .unwrap_or_default()
        .iter()
        .any(|ns| &ns.typ().to_string() == "user" && ns.path().is_none())
}

fn get_namespaces(linux: &Linux) -> Vec<LinuxNamespace> {
    linux
        .namespaces()
        .clone()
        .unwrap_or_default()
        .iter()
        .map(|ns| {
            let mut namespace = LinuxNamespace::default();
            namespace.set_typ(ns.typ());
            namespace.set_path(ns.path().clone());

            namespace
        })
        .collect()
}

pub fn setup_child_logger(
    log_file_stream: PipeStream,
    child_logger: Logger,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let buf_reader_stream = tokio::io::BufReader::new(log_file_stream);
        let mut lines = buf_reader_stream.lines();

        loop {
            match lines.next_line().await {
                Err(e) => {
                    info!(child_logger, "read child process log error: {:?}", e);
                    break;
                }
                Ok(Some(line)) => {
                    info!(child_logger, "{}", line);
                }
                Ok(None) => {
                    info!(child_logger, "read child process log end",);
                    break;
                }
            }
        }
    })
}

async fn join_namespaces(
    logger: &Logger,
    spec: &Spec,
    p: &Process,
    cm: &(dyn Manager + Send + Sync),
    pipe_w: &mut PipeStream,
    pipe_r: &mut PipeStream,
) -> Result<()> {
    let logger = logger.new(o!("action" => "join-namespaces"));

    let linux = spec
        .linux()
        .as_ref()
        .ok_or_else(|| anyhow!("Spec didn't contain linux field"))?;
    let res = linux.resources().as_ref();

    let userns = is_userns_enabled(linux);

    info!(logger, "try to send spec from parent to child");
    let spec_str = serde_json::to_string(spec)?;
    write_async(pipe_w, SYNC_DATA, spec_str.as_str()).await?;

    info!(logger, "wait child received oci spec");
    read_async(pipe_r).await?;

    info!(logger, "send oci process from parent to child");
    let process_str = serde_json::to_string(&p.oci)?;
    write_async(pipe_w, SYNC_DATA, process_str.as_str()).await?;

    info!(logger, "wait child received oci process");
    read_async(pipe_r).await?;

    // wait child setup user namespace
    info!(logger, "wait child setup user namespace");
    read_async(pipe_r).await?;

    if userns {
        info!(logger, "setup uid/gid mappings");
        let uid_mappings = linux.uid_mappings().clone().unwrap_or_default();
        let gid_mappings = linux.gid_mappings().clone().unwrap_or_default();
        // setup uid/gid mappings
        write_mappings(&logger, &format!("/proc/{}/uid_map", p.pid), &uid_mappings)?;
        write_mappings(&logger, &format!("/proc/{}/gid_map", p.pid), &gid_mappings)?;
    }

    // Apply cgroups before allowing the child to continue.
    if res.is_some() {
        info!(logger, "apply processes to cgroups!");
        cm.apply(p.pid)?;
    }

    if p.init {
        if let Some(resource) = res {
            info!(logger, "set properties to cgroups!");
            cm.set(resource)?;
        }
    }

    info!(logger, "notify child to continue");
    // notify child to continue
    write_async(pipe_w, SYNC_SUCCESS, "").await?;

    info!(logger, "wait for child process ready to run exec");
    read_async(pipe_r).await?;

    Ok(())
}

fn write_mappings(logger: &Logger, path: &str, maps: &[LinuxIdMapping]) -> Result<()> {
    let data = maps
        .iter()
        .filter(|m| m.size() != 0)
        .map(|m| format!("{} {} {}\n", m.container_id(), m.host_id(), m.size()))
        .collect::<Vec<_>>()
        .join("");

    info!(logger, "mapping: {}", data);
    if !data.is_empty() {
        let fd = fcntl::open(path, OFlag::O_WRONLY, Mode::empty())?;
        // OwnedFd will be automatically closed when dropped
        unistd::write(&fd, data.as_bytes())
            .inspect_err(|_| info!(logger, "cannot write mapping"))?;
    }
    Ok(())
}

fn setid(uid: Uid, gid: Gid) -> Result<()> {
    // set uid/gid
    capctl::prctl::set_keepcaps(true)
        .map_err(|e| anyhow!(e).context("set keep capabilities returned"))?;

    {
        unistd::setresgid(gid, gid, gid)?;
    }
    {
        unistd::setresuid(uid, uid, uid)?;
    }
    // if we change from zero, we lose effective caps
    if uid != Uid::from_raw(0) {
        capabilities::reset_effective()?;
    }

    capctl::prctl::set_keepcaps(false)
        .map_err(|e| anyhow!(e).context("set keep capabilities returned"))?;

    Ok(())
}

impl LinuxContainer {
    pub fn new<T: Into<String> + Display + Clone>(
        id: T,
        base: T,
        devcg_info: Option<Arc<RwLock<DevicesCgroupInfo>>>,
        config: Config,
        logger: &Logger,
    ) -> Result<Self> {
        let base = base.into();
        let id = id.into();
        let root = format!("{}/{}", base.as_str(), id.as_str());

        // validate oci spec
        validator::validate(&config)?;

        fs::create_dir_all(root.as_str()).map_err(|e| {
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                return anyhow!(e).context(format!("container {} already exists", id.as_str()));
            }

            anyhow!(e).context(format!("fail to create container directory {root}"))
        })?;

        unistd::chown(
            root.as_str(),
            Some(unistd::getuid()),
            Some(unistd::getgid()),
        )
        .context(format!("Cannot change owner of container {id} root"))?;

        let spec = config.spec.as_ref().unwrap();
        let linux_cgroups_path = spec
            .linux()
            .as_ref()
            .unwrap()
            .cgroups_path()
            .as_ref()
            .map_or(String::new(), |cgrp| cgrp.display().to_string());
        let cpath = if linux_cgroups_path.is_empty() {
            format!("/{}", id.as_str())
        } else {
            // if we have a systemd cgroup path we need to convert it to a fs cgroup path
            linux_cgroups_path.replace(':', "/")
        };

        let cgroup_manager: Arc<dyn Manager + Send + Sync> = Arc::new(
            FsManager::new(cpath.as_str(), spec, devcg_info).context("Create cgroupfs manager")?,
        );
        info!(logger, "new cgroup_manager {:?}", &cgroup_manager);

        Ok(LinuxContainer {
            id: id.clone(),
            root,
            cgroup_manager,
            status: ContainerStatus::new(),
            config,
            processes: HashMap::new(),
            init_process_pid: -1,
            logger: logger.new(o!("module" => "rustjail", "subsystem" => "container", "cid" => id)),
            pinned_namespace_fds: HashMap::new(),
        })
    }
}

use std::fs::OpenOptions;
use std::io::Write;

fn set_sysctls(sysctls: &HashMap<String, String>) -> Result<()> {
    for (key, value) in sysctls {
        let name = format!("/proc/sys/{}", key.replace('.', "/"));
        let mut file = match OpenOptions::new()
            .read(true)
            .write(true)
            .create(false)
            .open(name.as_str())
        {
            Ok(f) => f,
            Err(e) => {
                if e.kind() == std::io::ErrorKind::NotFound {
                    continue;
                }
                return Err(e.into());
            }
        };

        file.write_all(value.as_bytes())?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::Process;
    use nix::unistd::Uid;
    use oci::{
        LinuxBuilder, LinuxDeviceCgroupBuilder, LinuxNamespaceBuilder, LinuxResourcesBuilder, Root,
        SpecBuilder,
    };
    use oci_spec::runtime as oci;
    use std::fs;
    use std::os::unix::fs::MetadataExt;
    use std::os::unix::io::AsRawFd;
    use std::time::{SystemTime, UNIX_EPOCH};
    use tempfile::tempdir;
    use test_utils::skip_if_not_root;

    const CGROUP_PARENT: &str = "kata.agent.test.k8s.io";

    fn sl() -> slog::Logger {
        slog_scope::logger()
    }

    #[test]
    fn test_status_transtition() {
        let mut status = ContainerStatus::new();
        let status_table: [ContainerState; 4] = [
            ContainerState::Created,
            ContainerState::Running,
            ContainerState::Paused,
            ContainerState::Stopped,
        ];

        for s in status_table.iter() {
            status.transition(*s);
            assert_eq!(status.status(), *s);
        }
    }

    #[test]
    fn test_set_stdio_permissions() {
        skip_if_not_root!();

        let null_rdev = fs::metadata("/dev/null").unwrap().rdev();
        let fds = [
            std::io::stdin().as_raw_fd(),
            std::io::stdout().as_raw_fd(),
            std::io::stderr().as_raw_fd(),
        ];

        // SAFETY: the stdio fds stay open for the duration of the test.
        let old_uid = stat::fstat(unsafe { BorrowedFd::borrow_raw(fds[0]) })
            .unwrap()
            .st_uid;

        let uid = 1000;
        set_stdio_permissions(Uid::from_raw(uid)).unwrap();

        // Check the stdio fds themselves rather than the /dev/std* paths:
        // the fds may not point at those nodes (e.g. when stdio is
        // redirected), which is also why set_stdio_permissions operates on
        // the fds. Mirror its skipping of /dev/null backed fds.
        for fd in &fds {
            // SAFETY: the stdio fds stay open for the duration of the test.
            let stat = stat::fstat(unsafe { BorrowedFd::borrow_raw(*fd) }).unwrap();
            if stat.st_rdev == null_rdev {
                continue;
            }
            assert_eq!(stat.st_uid, uid);
        }

        // restore the uid
        set_stdio_permissions(Uid::from_raw(old_uid)).unwrap();
    }

    #[test]
    fn test_namespaces() {
        lazy_static::initialize(&NAMESPACES);
        assert_eq!(NAMESPACES.len(), 7);

        let ns = NAMESPACES.get("user");
        assert!(ns.is_some());

        let ns = NAMESPACES.get("ipc");
        assert!(ns.is_some());

        let ns = NAMESPACES.get("pid");
        assert!(ns.is_some());

        let ns = NAMESPACES.get("net");
        assert!(ns.is_some());

        let ns = NAMESPACES.get("mnt");
        assert!(ns.is_some());

        let ns = NAMESPACES.get("uts");
        assert!(ns.is_some());

        let ns = NAMESPACES.get("cgroup");
        assert!(ns.is_some());
    }

    #[test]
    fn test_typetoname() {
        lazy_static::initialize(&TYPETONAME);
        assert_eq!(TYPETONAME.len(), 7);

        let ns = TYPETONAME.get(&oci::LinuxNamespaceType::User);
        assert!(ns.is_some());

        let ns = TYPETONAME.get(&oci::LinuxNamespaceType::Ipc);
        assert!(ns.is_some());

        let ns = TYPETONAME.get(&oci::LinuxNamespaceType::Pid);
        assert!(ns.is_some());

        let ns = TYPETONAME.get(&oci::LinuxNamespaceType::Network);
        assert!(ns.is_some());

        let ns = TYPETONAME.get(&oci::LinuxNamespaceType::Mount);
        assert!(ns.is_some());

        let ns = TYPETONAME.get(&oci::LinuxNamespaceType::Uts);
        assert!(ns.is_some());

        let ns = TYPETONAME.get(&oci::LinuxNamespaceType::Cgroup);
        assert!(ns.is_some());
    }

    #[test]
    fn test_update_namespaces_pins_namespace_fds() {
        let mount_namespace = LinuxNamespaceBuilder::default()
            .typ(oci::LinuxNamespaceType::Mount)
            .build()
            .unwrap();
        let mut spec = SpecBuilder::default()
            .linux(
                LinuxBuilder::default()
                    .namespaces(vec![mount_namespace])
                    .build()
                    .unwrap(),
            )
            .build()
            .unwrap();
        let mut pinned_namespace_fds = HashMap::new();

        update_namespaces(
            &sl(),
            &mut spec,
            std::process::id() as RawFd,
            &mut pinned_namespace_fds,
        )
        .unwrap();

        let namespace = &spec
            .linux()
            .as_ref()
            .unwrap()
            .namespaces()
            .as_ref()
            .unwrap()[0];
        let pinned_path = namespace.path().as_ref().unwrap();
        let agent_pid = std::process::id();
        assert!(pinned_path.starts_with(format!("/proc/{agent_pid}/fd")));
        assert_eq!(pinned_namespace_fds.len(), 1);
        assert_eq!(
            fs::metadata(pinned_path).unwrap().ino(),
            fs::metadata("/proc/self/ns/mnt").unwrap().ino()
        );
    }

    #[test]
    fn test_pid_namespace_is_required_and_may_be_created_or_joined() {
        let logger = slog_scope::logger();
        let mut linux = Linux::default();
        linux.set_namespaces(None);
        assert!(get_pid_namespace(&logger, &linux).is_err());

        let mut namespace = LinuxNamespaceBuilder::default()
            .typ(oci::LinuxNamespaceType::Pid)
            .build()
            .unwrap();
        linux.set_namespaces(Some(vec![namespace.clone()]));
        assert_eq!(get_pid_namespace(&logger, &linux).unwrap(), None);

        namespace.set_path(Some(PathBuf::from("/proc/self/ns/pid")));
        linux.set_namespaces(Some(vec![namespace]));
        let fd = get_pid_namespace(&logger, &linux).unwrap().unwrap();
        let file = unsafe { fs::File::from_raw_fd(fd) };
        assert_eq!(
            file.metadata().unwrap().ino(),
            fs::metadata("/proc/self/ns/pid").unwrap().ino()
        );
    }

    fn create_dummy_opts() -> CreateOpts {
        let start = SystemTime::now();
        let since_the_epoch = start
            .duration_since(UNIX_EPOCH)
            .expect("Time went backwards");

        let mut root = Root::default();
        root.set_path(String::from("/tmp").into());

        let linux_resources = LinuxResourcesBuilder::default()
            .devices(vec![LinuxDeviceCgroupBuilder::default()
                .allow(true)
                .typ(oci::LinuxDeviceType::C)
                .access("rwm")
                .build()
                .unwrap()])
            .build()
            .unwrap();

        let cgroups_path = format!(
            "/{}/dummycontainer{}",
            CGROUP_PARENT,
            since_the_epoch.as_micros()
        );

        let mut spec = SpecBuilder::default()
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
        spec.set_process(None);

        CreateOpts {
            no_pivot_root: false,
            spec: Some(spec),
        }
    }

    fn new_linux_container() -> (Result<LinuxContainer>, tempfile::TempDir) {
        // Create a temporal directory
        let dir = tempdir()
            .map_err(|e| anyhow!(e).context("tempdir failed"))
            .unwrap();

        // Keep teardown tests inside their own directory: never detach a shared /tmp mount.
        let mut config = create_dummy_opts();
        config
            .spec
            .as_mut()
            .unwrap()
            .root_mut()
            .as_mut()
            .unwrap()
            .set_path(dir.path().to_path_buf());

        // Create a new container
        (
            LinuxContainer::new(
                "some_id",
                dir.path().join("rootfs").to_str().unwrap(),
                None,
                config,
                &slog_scope::logger(),
            ),
            dir,
        )
    }

    fn new_linux_container_and_then<U, F: FnOnce(LinuxContainer) -> Result<U, anyhow::Error>>(
        op: F,
    ) -> Result<U, anyhow::Error> {
        let (container, _dir) = new_linux_container();
        container.and_then(op)
    }

    #[test]
    fn test_linuxcontainer_pause_bad_status() {
        let ret = new_linux_container_and_then(|mut c: LinuxContainer| {
            // Change state to pause, c.pause() should fail
            c.status.transition(ContainerState::Paused);
            c.pause().map_err(|e| anyhow!(e))
        });

        assert!(ret.is_err(), "Expecting error, Got {:?}", ret);
        assert!(format!("{:?}", ret).contains("failed to pause container"))
    }

    #[test]
    #[ignore = "requires a delegated disposable cgroup; empty path targets the host root"]
    fn test_linuxcontainer_pause() {
        let ret = new_linux_container_and_then(|mut c: LinuxContainer| {
            c.cgroup_manager =
                Arc::new(FsManager::new("", &Spec::default(), None).map_err(|e| {
                    anyhow!(format!("fail to create cgroup manager with path: {:}", e))
                })?);
            c.pause().map_err(|e| anyhow!(e))
        });

        assert!(ret.is_ok(), "Expecting Ok, Got {:?}", ret);
    }

    #[test]
    fn test_linuxcontainer_resume_bad_status() {
        let ret = new_linux_container_and_then(|mut c: LinuxContainer| {
            // Change state to created, c.resume() should fail
            c.status.transition(ContainerState::Created);
            c.resume().map_err(|e| anyhow!(e))
        });

        assert!(ret.is_err(), "Expecting error, Got {:?}", ret);
        assert!(format!("{:?}", ret).contains("not paused"))
    }

    #[test]
    #[ignore = "requires a delegated disposable cgroup; empty path targets the host root"]
    fn test_linuxcontainer_resume() {
        let ret = new_linux_container_and_then(|mut c: LinuxContainer| {
            c.cgroup_manager =
                Arc::new(FsManager::new("", &Spec::default(), None).map_err(|e| {
                    anyhow!(format!("fail to create cgroup manager with path: {:}", e))
                })?);
            // Change status to paused, this way we can resume it
            c.status.transition(ContainerState::Paused);
            c.resume().map_err(|e| anyhow!(e))
        });

        assert!(ret.is_ok(), "Expecting Ok, Got {:?}", ret);
    }

    #[test]
    fn test_linuxcontainer_get_process_not_found() {
        let _ = new_linux_container_and_then(|mut c: LinuxContainer| {
            let p = c.get_process("123");
            assert!(p.is_err(), "Expecting Err, Got {:?}", p);
            Ok(())
        });
    }

    #[tokio::test]
    async fn test_linuxcontainer_get_process() {
        let _ = new_linux_container_and_then(|mut c: LinuxContainer| {
            let process = Process::new(&sl(), &oci::Process::default(), "123", true, 1).unwrap();
            let exec_id = process.exec_id.clone();
            c.processes.insert(exec_id, process);

            let p = c.get_process("123");
            assert!(p.is_ok(), "Expecting Ok, Got {:?}", p);
            Ok(())
        });
    }

    #[test]
    fn test_linuxcontainer_set() {
        let ret = new_linux_container_and_then(|mut c: LinuxContainer| {
            c.set(oci::LinuxResources::default())
        });
        assert!(ret.is_ok(), "Expecting Ok, Got {:?}", ret);
    }

    #[tokio::test]
    async fn test_linuxcontainer_start() {
        let (c, _dir) = new_linux_container();
        let mut oci_process = oci::Process::default();
        oci_process.set_capabilities(None);
        let ret = c
            .unwrap()
            .start(Process::new(&sl(), &oci_process, "123", true, 1).unwrap())
            .await;
        assert!(format!("{:?}", ret).contains("no process config"));
    }

    #[tokio::test]
    async fn test_linuxcontainer_run() {
        let (c, _dir) = new_linux_container();
        let mut oci_process = oci::Process::default();
        oci_process.set_capabilities(None);
        let ret = c
            .unwrap()
            .run(Process::new(&sl(), &oci_process, "123", true, 1).unwrap())
            .await;
        assert!(format!("{:?}", ret).contains("no process config"));
    }

    #[tokio::test]
    async fn test_linuxcontainer_destroy() {
        let (c, _dir) = new_linux_container();

        let ret = c.unwrap().destroy().await;
        assert!(ret.is_ok(), "Expecting Ok, Got {:?}", ret);
    }

    #[tokio::test]
    async fn test_linuxcontainer_exec() {
        let (c, _dir) = new_linux_container();
        let ret = c.unwrap().exec().await;
        assert!(ret.is_err(), "Expecting Err, Got {:?}", ret);
    }

    #[test]
    fn test_linuxcontainer_do_init_child() {
        let ret = do_init_child(std::io::stdin().as_raw_fd());
        assert!(ret.is_err(), "Expecting Err, Got {:?}", ret);
    }
}

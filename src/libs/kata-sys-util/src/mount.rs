// Copyright (c) 2019-2022 Alibaba Cloud
// Copyright (c) 2019-2021 Ant Group
//
// SPDX-License-Identifier: Apache-2.0
//

//! Utilities and helpers to execute mount operations on Linux systems.
//!
//! These utilities and helpers are specially designed and implemented to support container runtimes
//! on Linux systems, so they may not be generic enough.
//!
//! # Quotation from [mount(2)](https://man7.org/linux/man-pages/man2/mount.2.html)
//!
//! A call to mount() performs one of a number of general types of operation, depending on the bits
//! specified in mountflags. The choice of which operation to perform is determined by testing the
//! bits set in mountflags, with the tests being conducted in the order listed here:
//! - Remount an existing mount: mountflags includes MS_REMOUNT.
//! - Create a bind mount: mountflags includes MS_BIND.
//! - Change the propagation type of an existing mount: mountflags includes one of MS_SHARED,
//!   MS_PRIVATE, MS_SLAVE, or MS_UNBINDABLE.
//! - Move an existing mount to a new location: mountflags includes MS_MOVE.
//! - Create a new mount: mountflags includes none of the above flags.
//!
//! Since Linux 2.6.26, the MS_REMOUNT flag can be used with MS_BIND to modify only the
//! per-mount-point flags. This is particularly useful for setting or clearing the "read-only"
//! flag on a mount without changing the underlying filesystem. Specifying mountflags as:
//!            MS_REMOUNT | MS_BIND | MS_RDONLY
//! will make access through this mountpoint read-only, without affecting other mounts.
//!
//! # Safety
//!
//! Mount related operations are sensitive to security flaws, especially when dealing with symlinks.
//! There are several CVEs related to file path handling, for example
//! [CVE-2021-30465](https://github.com/opencontainers/runc/security/advisories/GHSA-c3xm-pvg7-gh7r).
//!
//! So some design rules are adopted here:
//! - both bind mount variants assume
//!   that all received paths are safe.
//! - the caller must ensure safe version of `PathBuf` are passed to mount variants.
//! - `create_mount_destination()` creates a mountpoint but does not constrain its path.
//! - the `safe_path` crate should be used to generate safe `PathBuf` for general cases.

use std::fmt::Debug;
use std::fs;
use std::io::{self, BufRead};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use lazy_static::lazy_static;
use nix::mount::{mount, MsFlags};
use nix::unistd;
use oci_spec::runtime as oci;

use crate::sl;

/// Default permission for directories created for mountpoint.
const MOUNT_DIR_PERM: u32 = 0o755;
const MOUNT_FILE_PERM: u32 = 0o644;

pub const PROC_MOUNTS_FILE: &str = "/proc/mounts";
const PROC_FIELDS_PER_LINE: usize = 6;
const PROC_PATH_INDEX: usize = 1;
const PROC_TYPE_INDEX: usize = 2;

lazy_static! {
    static ref MAX_MOUNT_PARAM_SIZE: usize =
        if let Ok(Some(v)) = unistd::sysconf(unistd::SysconfVar::PAGE_SIZE) {
            v as usize
        } else {
            panic!("cannot get PAGE_SIZE by sysconf()");
        };
}

/// Errors related to filesystem mount operations.
#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("Can not bind mount {0} to {1}: {2}")]
    BindMount(PathBuf, PathBuf, nix::Error),
    #[error("Failure injection: {0}")]
    FailureInject(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("Invalid mountpoint entry (expected {0} fields, got {1}) fields: {2}")]
    InvalidMountEntry(usize, usize, String),
    #[error("Invalid mount option: {0}")]
    InvalidMountOption(String),
    #[error("Invalid path: {0}")]
    InvalidPath(PathBuf),
    #[error("Can not mount {0} to {1}: {2}")]
    Mount(PathBuf, PathBuf, nix::Error),
    #[error("Mount option exceeds 4K size")]
    MountOptionTooBig,
    #[error("Path for mountpoint is null")]
    NullMountPointPath,
    #[error("Invalid Propagation type Flag")]
    InvalidPgMountFlag,
    #[error("Can not remount {0}: {1}")]
    Remount(PathBuf, nix::Error),
    #[error("Can not find mountpoint for {0}")]
    NoMountEntry(String),
}

/// A specialized version of `std::result::Result` for mount operations.
pub type Result<T> = std::result::Result<T, Error>;

/// Information of mount record from `/proc/mounts`.
pub struct LinuxMountInfo {
    /// Filesystem type of mount, third field of records from `/proc/mounts`.
    pub fs_type: String,
}

/// Get the file system type of a mount point by parsing `/proc/mounts`.
pub fn get_linux_mount_info(mount_point: &str) -> Result<LinuxMountInfo> {
    let mount_file = fs::File::open(PROC_MOUNTS_FILE)?;
    let reader = io::BufReader::new(mount_file);

    for line in reader.lines() {
        let mount = line?;
        let fields: Vec<&str> = mount.split(' ').collect();

        if fields.len() != PROC_FIELDS_PER_LINE {
            return Err(Error::InvalidMountEntry(
                PROC_FIELDS_PER_LINE,
                fields.len(),
                mount,
            ));
        }

        if mount_point == fields[PROC_PATH_INDEX] {
            return Ok(LinuxMountInfo {
                fs_type: fields[PROC_TYPE_INDEX].to_string(),
            });
        }
    }

    Err(Error::NoMountEntry(mount_point.to_owned()))
}

/// Recursively create destination for a mount.
///
/// For a normal mount, the destination will always be a directory. For bind mount, the destination
/// must be a directory if the source is a directory, otherwise the destination must be a normal
/// file. If directories are created, their permissions are initialized to MountPerm.
///
/// # Safety
///
/// The caller must validate the destination and its ancestors. This function does
/// not confine paths to a root directory or protect against symlink replacement.
pub fn create_mount_destination<S: AsRef<Path>, D: AsRef<Path>>(
    src: S,
    dst: D,
    fs_type: &str,
) -> Result<impl AsRef<Path> + Debug> {
    let dst = dst.as_ref();
    let parent = dst
        .parent()
        .ok_or_else(|| Error::InvalidPath(dst.to_path_buf()))?;
    let mut builder = fs::DirBuilder::new();
    builder.mode(MOUNT_DIR_PERM).recursive(true);

    // Try to create parent directory, but handle ENOSYS gracefully
    // ENOSYS can occur on certain filesystems (e.g., virtio-fs) where mkdir is not fully supported
    if let Err(e) = builder.create(parent) {
        // If the error is ENOSYS or AlreadyExists, check if parent exists and continue
        if e.kind() != std::io::ErrorKind::AlreadyExists && e.raw_os_error() != Some(libc::ENOSYS) {
            return Err(e.into());
        }
        // Verify parent exists
        if !parent.exists() {
            return Err(e.into());
        }
    }

    if fs_type == "bind" {
        // The source and destination for bind mounting must be the same type: file or directory.
        if !src.as_ref().is_dir() {
            fs::OpenOptions::new()
                .mode(MOUNT_FILE_PERM)
                .write(true)
                .create(true)
                .open(dst)?;
            return Ok(dst.to_path_buf());
        }
    }

    // Try to create destination directory, but handle ENOSYS gracefully
    if let Err(e) = builder.create(dst) {
        if e.kind() != std::io::ErrorKind::AlreadyExists && e.raw_os_error() != Some(libc::ENOSYS) {
            return Err(e.into());
        }
        // If ENOSYS or AlreadyExists, check if dst exists and is a directory
        if !dst.exists() || !dst.is_dir() {
            return Err(Error::InvalidPath(dst.to_path_buf()));
        }
    }

    if !dst.is_dir() {
        Err(Error::InvalidPath(dst.to_path_buf()))
    } else {
        Ok(dst.to_path_buf())
    }
}

/// Remount a bind mount
///
/// # Safety
/// Caller needs to ensure safety of the `dst` to avoid possible file path based attacks.
pub fn bind_remount<P: AsRef<Path>>(dst: P, readonly: bool) -> Result<()> {
    let dst = dst.as_ref();
    if dst.as_os_str().is_empty() {
        return Err(Error::NullMountPointPath);
    }
    let dst = dst
        .canonicalize()
        .map_err(|_e| Error::InvalidPath(dst.to_path_buf()))?;

    do_rebind_mount(dst, readonly, MsFlags::empty())
}

/// Bind mount `src` to `dst` with a custom propagation type, optionally in readonly mode if
/// `readonly` is true.
///
/// Propagation type: MsFlags::MS_SHARED or MsFlags::MS_SLAVE
/// MsFlags::MS_SHARED is used to bind mount the sandbox path to enable `exec` (in case of FC
/// jailer).
/// MsFlags::MS_SLAVE is used on all other cases.
///
/// # Safety
/// Caller needs to ensure:
/// - `src` exists.
/// - `dst` exists, and is suitable as destination for bind mount.
/// - `dst` is free of file path based attacks.
pub fn bind_mount_unchecked<S: AsRef<Path>, D: AsRef<Path>>(
    src: S,
    dst: D,
    readonly: bool,
    pgflag: MsFlags,
) -> Result<()> {
    fail::fail_point!("bind_mount", |_| {
        Err(Error::FailureInject(
            "Bind mount fail point injection".to_string(),
        ))
    });

    let src = src.as_ref();
    let dst = dst.as_ref();
    if src.as_os_str().is_empty() {
        return Err(Error::NullMountPointPath);
    }
    if dst.as_os_str().is_empty() {
        return Err(Error::NullMountPointPath);
    }
    let abs_src = src
        .canonicalize()
        .map_err(|_e| Error::InvalidPath(src.to_path_buf()))?;

    create_mount_destination(src, dst, "bind")?;
    // Bind mount `src` to `dst`.
    mount(
        Some(&abs_src),
        dst,
        Some("bind"),
        MsFlags::MS_BIND,
        Some(""),
    )
    .map_err(|e| Error::BindMount(abs_src, dst.to_path_buf(), e))?;

    // Change into the chosen propagation mode.
    if !(pgflag == MsFlags::MS_SHARED || pgflag == MsFlags::MS_SLAVE) {
        return Err(Error::InvalidPgMountFlag);
    }
    mount(Some(""), dst, Some(""), pgflag, Some(""))
        .map_err(|e| Error::Mount(PathBuf::new(), dst.to_path_buf(), e))?;

    // Optionally rebind into readonly mode.
    if readonly {
        do_rebind_mount(dst, readonly, MsFlags::empty())?;
    }

    Ok(())
}

#[inline]
fn do_rebind_mount<P: AsRef<Path>>(path: P, readonly: bool, flags: MsFlags) -> Result<()> {
    mount(
        Some(""),
        path.as_ref(),
        Some(""),
        if readonly {
            flags | MsFlags::MS_BIND | MsFlags::MS_REMOUNT | MsFlags::MS_RDONLY
        } else {
            flags | MsFlags::MS_BIND | MsFlags::MS_REMOUNT
        },
        Some(""),
    )
    .map_err(|e| Error::Remount(path.as_ref().to_path_buf(), e))
}

/// Take fstab style mount options and parses them for use with a standard mount() syscall.
pub fn parse_mount_options<T: AsRef<str>>(options: &[T]) -> Result<(MsFlags, String)> {
    let mut flags: MsFlags = MsFlags::empty();
    let mut data: Vec<String> = Vec::new();

    for opt in options.iter() {
        if opt.as_ref() == "loop" {
            return Err(Error::InvalidMountOption("loop".to_string()));
        } else if let Some(v) = parse_mount_flags(flags, opt.as_ref()) {
            flags = v;
        } else {
            data.push(opt.as_ref().to_string());
        }
    }

    let data = data.join(",");
    if data.len() > *MAX_MOUNT_PARAM_SIZE {
        return Err(Error::MountOptionTooBig);
    }

    Ok((flags, data))
}

fn parse_mount_flags(mut flags: MsFlags, flag_str: &str) -> Option<MsFlags> {
    // Following mount options are applicable to fstab only.
    // - _netdev: The filesystem resides on a device that requires network access (used to prevent
    //   the system from attempting to mount these filesystems until the network has been enabled
    //   on the system).
    // - auto: Can be mounted with the -a option.
    // - group: Allow an ordinary user to mount the filesystem if one of that user’s groups matches
    //   the group of the device. This option implies the options nosuid and nodev (unless
    //    overridden by subsequent options, as in the option line group,dev,suid).
    // - noauto: Can only be mounted explicitly (i.e., the -a option will not cause the filesystem
    //   to be mounted).
    // - nofail: Do not report errors for this device if it does not exist.
    // - owner: Allow an ordinary user to mount the filesystem if that user is the owner of the
    //   device. This option implies the options nosuid and nodev (unless overridden by subsequent
    //   options, as in the option line owner,dev,suid).
    // - user: Allow an ordinary user to mount the filesystem. The name of the mounting user is
    //   written to the mtab file (or to the private libmount file in /run/mount on systems without
    //   a regular mtab) so that this same user can unmount the filesystem again. This option
    //   implies the options noexec, nosuid, and nodev (unless overridden by subsequent options,
    //   as in the option line user,exec,dev,suid).
    // - nouser: Forbid an ordinary user to mount the filesystem. This is the default; it does not
    //   imply any other options.
    // - users: Allow any user to mount and to unmount the filesystem, even when some other ordinary
    //   user mounted it. This option implies the options noexec, nosuid, and nodev (unless
    //   overridden by subsequent options, as in the option line users,exec,dev,suid).
    match flag_str {
        // Clear flags
        "defaults" => {}
        "async" => flags &= !MsFlags::MS_SYNCHRONOUS,
        "atime" => flags &= !MsFlags::MS_NOATIME,
        "dev" => flags &= !MsFlags::MS_NODEV,
        "diratime" => flags &= !MsFlags::MS_NODIRATIME,
        "exec" => flags &= !MsFlags::MS_NOEXEC,
        "loud" => flags &= !MsFlags::MS_SILENT,
        "noiversion" => flags &= !MsFlags::MS_I_VERSION,
        "nomand" => flags &= !MsFlags::MS_MANDLOCK,
        "norelatime" => flags &= !MsFlags::MS_RELATIME,
        "nostrictatime" => flags &= !MsFlags::MS_STRICTATIME,
        "rw" => flags &= !MsFlags::MS_RDONLY,
        "suid" => flags &= !MsFlags::MS_NOSUID,
        // Set flags
        "bind" => flags |= MsFlags::MS_BIND,
        "dirsync" => flags |= MsFlags::MS_DIRSYNC,
        "iversion" => flags |= MsFlags::MS_I_VERSION,
        "mand" => flags |= MsFlags::MS_MANDLOCK,
        "noatime" => flags |= MsFlags::MS_NOATIME,
        "nodev" => flags |= MsFlags::MS_NODEV,
        "nodiratime" => flags |= MsFlags::MS_NODIRATIME,
        "noexec" => flags |= MsFlags::MS_NOEXEC,
        "nosuid" => flags |= MsFlags::MS_NOSUID,
        "rbind" => flags |= MsFlags::MS_BIND | MsFlags::MS_REC,
        "unbindable" => flags |= MsFlags::MS_UNBINDABLE,
        "runbindable" => flags |= MsFlags::MS_UNBINDABLE | MsFlags::MS_REC,
        "private" => flags |= MsFlags::MS_PRIVATE,
        "rprivate" => flags |= MsFlags::MS_PRIVATE | MsFlags::MS_REC,
        "shared" => flags |= MsFlags::MS_SHARED,
        "rshared" => flags |= MsFlags::MS_SHARED | MsFlags::MS_REC,
        "slave" => flags |= MsFlags::MS_SLAVE,
        "rslave" => flags |= MsFlags::MS_SLAVE | MsFlags::MS_REC,
        "relatime" => flags |= MsFlags::MS_RELATIME,
        "remount" => flags |= MsFlags::MS_REMOUNT,
        "ro" => flags |= MsFlags::MS_RDONLY,
        "silent" => flags |= MsFlags::MS_SILENT,
        "strictatime" => flags |= MsFlags::MS_STRICTATIME,
        "sync" => flags |= MsFlags::MS_SYNCHRONOUS,
        flag_str => {
            warn!(sl!(), "BUG: unknown mount flag: {:?}", flag_str);
            return None;
        }
    }
    Some(flags)
}

pub fn get_mount_path(p: &Option<PathBuf>) -> String {
    p.clone().unwrap_or_default().display().to_string()
}

pub fn get_mount_options(options: &Option<Vec<String>>) -> Vec<String> {
    match options {
        Some(o) => o.to_vec(),
        None => vec![],
    }
}

pub fn get_mount_type(m: &oci::Mount) -> String {
    m.typ()
        .clone()
        .map(|typ| {
            if typ.as_str() == "none" {
                if let Some(opts) = m.options() {
                    if opts.iter().any(|opt| opt == "bind" || opt == "rbind") {
                        return "bind".to_string();
                    }
                }
            }
            typ
        })
        .unwrap_or("bind".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_get_linux_mount_info() {
        let info = get_linux_mount_info("/dev/shm").unwrap();

        assert_eq!(&info.fs_type, "tmpfs");

        assert!(matches!(
            get_linux_mount_info(""),
            Err(Error::NoMountEntry(_))
        ));
        assert!(matches!(
            get_linux_mount_info("/sys/fs/cgroup/do_not_exist/____hi"),
            Err(Error::NoMountEntry(_))
        ));
    }

    #[test]
    fn test_create_mount_destination() {
        let tmpdir = tempfile::tempdir().unwrap();
        let src = Path::new("/proc/mounts");
        let mut dst = tmpdir.path().to_owned();
        dst.push("proc");
        dst.push("mounts");
        let dst = create_mount_destination(src, dst.as_path(), "bind").unwrap();
        let abs_dst = dst.as_ref().canonicalize().unwrap();
        assert!(abs_dst.is_file());

        let dst = Path::new("/");
        assert!(matches!(
            create_mount_destination(src, dst, "bind"),
            Err(Error::InvalidPath(_))
        ));

        let src = Path::new("/proc");
        let dst = Path::new("/proc/mounts");
        assert!(matches!(
            create_mount_destination(src, dst, "bind"),
            Err(Error::InvalidPath(_))
        ));
    }

    #[test]
    #[ignore]
    fn test_bind_remount() {
        let tmpdir = tempfile::tempdir().unwrap();
        let tmpdir2 = tempfile::tempdir().unwrap();

        assert!(matches!(
            bind_remount(PathBuf::from(""), true),
            Err(Error::NullMountPointPath)
        ));
        assert!(matches!(
            bind_remount(PathBuf::from("../______doesn't____exist____nnn"), true),
            Err(Error::InvalidPath(_))
        ));

        bind_mount_unchecked(tmpdir2.path(), tmpdir.path(), true, MsFlags::MS_SLAVE).unwrap();
        bind_remount(tmpdir.path(), true).unwrap();
        nix::mount::umount(tmpdir.path()).unwrap();
    }

    #[test]
    #[ignore]
    fn test_bind_mount() {
        let tmpdir = tempfile::tempdir().unwrap();
        let tmpdir2 = tempfile::tempdir().unwrap();
        let mut src = tmpdir.path().to_owned();
        src.push("src");
        let mut dst = tmpdir.path().to_owned();
        dst.push("src");

        assert!(matches!(
            bind_mount_unchecked(Path::new(""), Path::new(""), false, MsFlags::MS_SLAVE),
            Err(Error::NullMountPointPath)
        ));
        assert!(matches!(
            bind_mount_unchecked(tmpdir2.path(), Path::new(""), false, MsFlags::MS_SLAVE),
            Err(Error::NullMountPointPath)
        ));
        assert!(matches!(
            bind_mount_unchecked(
                Path::new("/_does_not_exist_/___aahhhh"),
                Path::new("/tmp/_does_not_exist/___bbb"),
                false,
                MsFlags::MS_SLAVE
            ),
            Err(Error::InvalidPath(_))
        ));

        let dst = create_mount_destination(tmpdir2.path(), &dst, "bind").unwrap();
        bind_mount_unchecked(tmpdir2.path(), dst.as_ref(), true, MsFlags::MS_SLAVE).unwrap();
        bind_mount_unchecked(&src, dst.as_ref(), false, MsFlags::MS_SLAVE).unwrap();
        nix::mount::umount(dst.as_ref()).unwrap();
        nix::mount::umount(dst.as_ref()).unwrap();

        let mut src = tmpdir.path().to_owned();
        src.push("file");
        fs::write(&src, "test").unwrap();
        let mut dst = tmpdir.path().to_owned();
        dst.push("file");
        let dst = create_mount_destination(&src, &dst, "bind").unwrap();
        bind_mount_unchecked(&src, dst.as_ref(), false, MsFlags::MS_SLAVE).unwrap();
        assert!(dst.as_ref().is_file());
        nix::mount::umount(dst.as_ref()).unwrap();
    }

    #[test]
    fn test_parse_mount_options() {
        let options: Vec<&str> = vec![];
        let (flags, data) = parse_mount_options(&options).unwrap();
        assert!(flags.is_empty());
        assert!(data.is_empty());

        let mut options = vec![
            "dev".to_string(),
            "ro".to_string(),
            "defaults".to_string(),
            "data-option".to_string(),
        ];
        let (flags, data) = parse_mount_options(&options).unwrap();
        assert_eq!(flags, MsFlags::MS_RDONLY);
        assert_eq!(&data, "data-option");

        options.push("loop".to_string());
        assert!(parse_mount_options(&options).is_err());

        let idx = options.len() - 1;
        options[idx] = " ".repeat(*MAX_MOUNT_PARAM_SIZE + 1);
        assert!(parse_mount_options(&options).is_err());
    }
}

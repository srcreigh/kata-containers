// Copyright (c) 2019-2021 Alibaba Cloud
// Copyright (c) 2019-2021 Ant Group
//
// SPDX-License-Identifier: Apache-2.0
//

#[cfg(feature = "safe-path")]
use anyhow::{anyhow, Context, Result};
#[cfg(feature = "safe-path")]
use base64::Engine as _;
use std::{collections::HashMap, path::PathBuf};

/// Prefix to mark a volume as Kata special.
pub const KATA_VOLUME_TYPE_PREFIX: &str = "kata:";

/// The Mount should be ignored by the host and handled by the guest.
pub const KATA_GUEST_MOUNT_PREFIX: &str = "kata:guest-mount:";

/// The sharedfs volume is mounted by guest OS before starting the kata-agent.
pub const KATA_SHAREDFS_GUEST_PREMOUNT_TAG: &str = "kataShared";

/// KATA_EPHEMERAL_VOLUME_TYPE creates a tmpfs backed volume for sharing files between containers.
pub const KATA_EPHEMERAL_VOLUME_TYPE: &str = "ephemeral";

/// KATA_K8S_LOCAL_STORAGE_TYPE is used for k8s empty dir (a disk-backed volume),
/// to create a local directory inside the VM for sharing files between containers.
pub const KATA_K8S_LOCAL_STORAGE_TYPE: &str = "local";

/// KATA_MOUNT_INFO_FILE_NAME is used for the file that holds direct-volume mount info
pub const KATA_MOUNT_INFO_FILE_NAME: &str = "mountInfo.json";

/// Specify `fsgid` for a volume or mount, `fsgid=1`.
pub const KATA_MOUNT_OPTION_FS_GID: &str = "fsgid";

/// DEFAULT_KATA_DIRECT_VOLUME_ROOT_PATH is the default root path used for concatenating with the direct-volume mount info file path
pub const DEFAULT_KATA_DIRECT_VOLUME_ROOT_PATH: &str = "/run/kata-containers/shared/direct-volumes";

/// Key to request filesystem creation for a fresh block volume.
pub const KATA_BLOCK_VOLUME_CREATE_FS: &str = "create_filesystem";

/// SANDBOX_BIND_MOUNTS_DIR is for sandbox bindmounts
pub const SANDBOX_BIND_MOUNTS_DIR: &str = "sandbox-mounts";

/// SANDBOX_BIND_MOUNTS_RO is for sandbox bindmounts with readonly
pub const SANDBOX_BIND_MOUNTS_RO: &str = ":ro";

/// KATA_VIRTUAL_VOLUME_PREFIX is for container image guest pull
pub const KATA_VIRTUAL_VOLUME_PREFIX: &str = "io.katacontainers.volume=";

/// kata default guest sandbox dir.
pub const DEFAULT_KATA_GUEST_SANDBOX_DIR: &str = "/run/kata-containers/sandbox";
/// default shm directory name.
pub const SHM_DIR: &str = "shm";
/// shm device path.
pub const SHM_DEVICE: &str = "/dev/shm";

/// Get the root path used for concatenating with the direct-volume mount info file path.
pub fn kata_direct_volume_root_path() -> String {
    DEFAULT_KATA_DIRECT_VOLUME_ROOT_PATH.to_string()
}

/// Get the sandbox bind mounts directory.
pub fn kata_guest_sandbox_dir() -> String {
    DEFAULT_KATA_GUEST_SANDBOX_DIR.to_string()
}

/// Information about a mount.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct Mount {
    /// A device name, but can also be a file or directory name for bind mounts or a dummy.
    /// Path values for bind mounts are either absolute or relative to the bundle. A mount is a
    /// bind mount if it has either bind or rbind in the options.
    pub source: String,
    /// Destination of mount point: path inside container. This value MUST be an absolute path.
    pub destination: PathBuf,
    /// The type of filesystem for the mountpoint.
    pub fs_type: String,
    /// Mount options for the mountpoint.
    pub options: Vec<String>,
    /// Optional device id for the block device when:
    /// - the source is a block device or a mountpoint for a block device
    /// - block device direct assignment is enabled
    pub device_id: Option<String>,
    /// Intermediate path to mount the source on host side and then passthrough to vm by shared fs.
    pub host_shared_fs_path: Option<PathBuf>,
    /// Whether to mount the mountpoint in readonly mode
    pub read_only: bool,
}

/// DirectVolumeMountInfo contains the information needed by Kata
/// to consume a host block device and mount it as a filesystem inside the guest VM.
#[derive(Debug, Clone, Eq, PartialEq, Default, Serialize, Deserialize)]
pub struct DirectVolumeMountInfo {
    /// The type of the volume (ie. block)
    #[serde(rename = "volume-type")]
    pub volume_type: String,
    /// The device backing the volume.
    pub device: String,
    /// The filesystem type to be mounted on the volume.
    #[serde(rename = "fstype")]
    pub fs_type: String,
    /// Additional metadata to pass to the agent regarding this volume.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, String>,
    /// Additional mount options.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
}

/// Joins a user-provided volume path with the Kata direct-volume root path.
///
/// The `volume_path` is base64-url-encoded and then safely joined to the `prefix`.
/// `safe_path` is OS-specific.
#[cfg(feature = "safe-path")]
pub fn join_path(prefix: &str, volume_path: &str) -> Result<PathBuf> {
    if volume_path.is_empty() {
        return Err(anyhow!(std::io::ErrorKind::NotFound));
    }
    let b64_url_encoded_path =
        base64::engine::general_purpose::URL_SAFE.encode(volume_path.as_bytes());

    Ok(safe_path::scoped_join(prefix, b64_url_encoded_path)?)
}

/// Gets `DirectVolumeMountInfo` from `mountinfo.json`.
/// `safe_path` is OS-specific.
#[cfg(feature = "safe-path")]
pub fn get_volume_mount_info(volume_path: &str) -> Result<DirectVolumeMountInfo> {
    let volume_path = join_path(kata_direct_volume_root_path().as_str(), volume_path)?;
    let mount_info_file_path = volume_path.join(KATA_MOUNT_INFO_FILE_NAME);
    let mount_info_file = std::fs::read_to_string(mount_info_file_path)?;
    let mount_info: DirectVolumeMountInfo = serde_json::from_str(&mount_info_file)?;

    Ok(mount_info)
}

/// Writes a `DirectVolumeMountInfo` as `mountInfo.json` under the direct-volume root
/// for the given `volume_path`.
#[cfg(feature = "safe-path")]
pub fn add_volume_mount_info(volume_path: &str, mount_info: &DirectVolumeMountInfo) -> Result<()> {
    let root = kata_direct_volume_root_path();
    // safe_path::scoped_join requires the root to exist; ensure it does before
    // calling join_path (mirrors Go's os.MkdirAll behaviour in AddMountInfo).
    std::fs::create_dir_all(&root)
        .with_context(|| format!("failed to create direct-volume root {:?}", root))?;
    let dir = join_path(&root, volume_path)?;
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("failed to create direct-volume dir {:?}", dir))?;
    let file_path = dir.join(KATA_MOUNT_INFO_FILE_NAME);
    let data =
        serde_json::to_string(mount_info).context("failed to serialize DirectVolumeMountInfo")?;
    std::fs::write(&file_path, data)
        .with_context(|| format!("failed to write mount info to {:?}", file_path))?;
    Ok(())
}

/// Returns `true` if a `mountInfo.json` exists for the given `volume_path`.
#[cfg(feature = "safe-path")]
pub fn is_volume_mounted(volume_path: &str) -> bool {
    get_volume_mount_info(volume_path).is_ok()
}

/// Removes the direct-volume metadata directory for the given `volume_path`.
#[cfg(feature = "safe-path")]
pub fn remove_volume_path(volume_path: &str) -> Result<()> {
    let dir = join_path(kata_direct_volume_root_path().as_str(), volume_path)?;
    if dir.exists() {
        std::fs::remove_dir_all(&dir)
            .with_context(|| format!("failed to remove direct-volume dir {:?}", dir))?;
    }
    Ok(())
}

/// Checks whether a mount type is a marker for a Kata specific volume.
pub fn is_kata_special_volume(ty: &str) -> bool {
    ty.len() > KATA_VOLUME_TYPE_PREFIX.len() && ty.starts_with(KATA_VOLUME_TYPE_PREFIX)
}

/// Checks whether a mount type is a marker for a Kata guest mount volume.
pub fn is_kata_guest_mount_volume(ty: &str) -> bool {
    ty.len() > KATA_GUEST_MOUNT_PREFIX.len() && ty.starts_with(KATA_GUEST_MOUNT_PREFIX)
}

/// Checks whether a mount type is a marker for a Kata ephemeral volume.
pub fn is_kata_ephemeral_volume(ty: &str) -> bool {
    ty == KATA_EPHEMERAL_VOLUME_TYPE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_kata_guest_sandbox_dir() {
        assert_eq!(kata_guest_sandbox_dir(), DEFAULT_KATA_GUEST_SANDBOX_DIR);
    }

    #[test]
    fn test_is_kata_special_volume() {
        assert!(is_kata_special_volume("kata:guest-mount:nfs"));
        assert!(!is_kata_special_volume("kata:"));
    }

    #[test]
    fn test_is_kata_guest_mount_volume() {
        assert!(is_kata_guest_mount_volume("kata:guest-mount:nfs"));
        assert!(!is_kata_guest_mount_volume("kata:guest-mount"));
        assert!(!is_kata_guest_mount_volume("kata:guest-moun"));
        assert!(!is_kata_guest_mount_volume("Kata:guest-mount:nfs"));
    }
}

// Copyright (c) 2019-2022 Alibaba Cloud
// Copyright (c) 2019-2022 Ant Group
//
// SPDX-License-Identifier: Apache-2.0
//

//! Paths for block mounts and files copied into the guest. No filesystem transport.
use std::path::Path;

pub(crate) const DEFAULT_KATA_GUEST_SHARE_DIR: &str = "/run/kata-containers/shared/containers/";
pub(crate) const PASSTHROUGH_FS_DIR: &str = "passthrough";

pub fn kata_guest_share_dir() -> String {
    DEFAULT_KATA_GUEST_SHARE_DIR.to_string()
}
pub fn ephemeral_path() -> String {
    "/run/kata-containers/sandbox/ephemeral".to_string()
}
pub fn do_get_guest_path(target: &str, cid: &str, is_volume: bool) -> String {
    let base = Path::new(DEFAULT_KATA_GUEST_SHARE_DIR).join(PASSTHROUGH_FS_DIR);
    let base = if is_volume { base } else { base.join(cid) };
    base.join(target).to_str().unwrap().to_string()
}

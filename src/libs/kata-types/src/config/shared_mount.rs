// Copyright (c) 2023
//
// SPDX-License-Identifier: Apache-2.0
//

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct SharedMount {
    /// Name is used to identify a pair of shared mount points.
    /// This field cannot be omitted.
    #[serde(default)]
    pub name: String,

    /// Src_ctr is used to specify the name of the source container.
    /// This field cannot be omitted.
    #[serde(default)]
    pub src_ctr: String,

    /// Src_path is used to specify the path to the shared mount point in the source container.
    /// Src_path must conform to the regular expression `^(/[-\w.]+)+/?$` and cannot contain `/../`.
    /// This field cannot be omitted.
    #[serde(default)]
    pub src_path: String,

    /// Dst_ctr is used to specify the name of the destination container.
    /// This field cannot be omitted.
    #[serde(default)]
    pub dst_ctr: String,

    /// Dst_path is used to specify the destination path where the shared mount point will be mounted.
    /// Dst_path must conform to the regular expression `^(/[-\w.]+)+/?$` and cannot contain `/../`.
    /// This field cannot be omitted.
    #[serde(default)]
    pub dst_path: String,
}

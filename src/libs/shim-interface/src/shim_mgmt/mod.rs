// Copyright (c) 2022 Alibaba Cloud
//
// SPDX-License-Identifier: Apache-2.0
//

/// The key for direct volume path
pub const DIRECT_VOLUME_PATH_KEY: &str = "path";
/// URL for stats direct volume
pub const DIRECT_VOLUME_STATS_URL: &str = "/direct-volume/stats";
/// URL for resizing direct volume
pub const DIRECT_VOLUME_RESIZE_URL: &str = "/direct-volume/resize";
/// URL for querying agent's socket
pub const AGENT_URL: &str = "/agent-url";
/// URL for querying metrics inside shim
pub const METRICS_URL: &str = "/metrics";
/// Untrusted guest-only metrics, kept separate from host metrics.
pub const GUEST_METRICS_URL: &str = "/metrics/guest";
/// URL for setting agent policy
pub const AGENT_POLICY_URL: &str = "/policy";

pub const ERR_NO_SHIM_SERVER: &str = "Failed to create shim management server";

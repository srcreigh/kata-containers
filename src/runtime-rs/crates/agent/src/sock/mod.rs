// Copyright (c) 2019-2022 Alibaba Cloud
// Copyright (c) 2019-2022 Ant Group
// SPDX-License-Identifier: Apache-2.0

mod hybrid_vsock;
use anyhow::{ensure, Context, Result};
use hybrid_vsock::HybridVsock;
use url::Url;

#[derive(Debug)]
pub struct ConnectConfig {
    dial_timeout_ms: u64,
    reconnect_timeout_ms: u64,
}

impl ConnectConfig {
    pub fn new(dial_timeout_ms: u64, reconnect_timeout_ms: u64) -> Self {
        Self {
            dial_timeout_ms,
            reconnect_timeout_ms: reconnect_timeout_ms.max(10_000),
        }
    }
}

pub fn new(address: &str, port: u32) -> Result<HybridVsock> {
    let url = Url::parse(address).context("parse agent URL")?;
    ensure!(
        url.scheme() == "hvsock",
        "kata-fc: only hybrid-vsock agent transport is supported"
    );
    ensure!(
        url.host_str().is_none() && !url.path().is_empty() && !url.path().contains(':'),
        "invalid hybrid-vsock path"
    );
    Ok(HybridVsock::new(url.path(), port))
}

#[cfg(test)]
mod tests {
    #[test]
    fn only_firecracker_transport_is_accepted() {
        assert!(super::new("hvsock:///run/kata/kata.hvsock", 1024).is_ok());
        for address in [
            "vsock://3",
            "remote:///tmp/agent.sock",
            "hvsock:///tmp/agent.sock:1024",
            "hvsock://host/tmp/agent.sock",
        ] {
            assert!(super::new(address, 1024).is_err(), "{address}");
        }
    }
}

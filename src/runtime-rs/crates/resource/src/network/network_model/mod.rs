// Copyright (c) 2019-2022 Alibaba Cloud
// Copyright (c) 2019-2022 Ant Group
//
// SPDX-License-Identifier: Apache-2.0
//

pub mod tc_filter_model;
use std::sync::Arc;

use anyhow::{Context, Result};
use async_trait::async_trait;

use super::NetworkPair;

pub(crate) const TC_FILTER_NET_MODEL_STR: &str = "tcfilter";

#[async_trait]
pub trait NetworkModel: std::fmt::Debug + Send + Sync {
    async fn add(&self, net_pair: &NetworkPair) -> Result<()>;
    async fn del(&self, net_pair: &NetworkPair) -> Result<()>;
}

pub fn new(model: &str) -> Result<Arc<dyn NetworkModel>> {
    anyhow::ensure!(
        model == TC_FILTER_NET_MODEL_STR,
        "kata-fc: only tcfilter networking is supported"
    );
    Ok(Arc::new(
        tc_filter_model::TcFilterModel::new().context("new tc filter model")?,
    ))
}

#[cfg(test)]
mod tests {
    #[test]
    fn unsupported_models_fail_before_network_changes() {
        for model in ["none", "l3forwarding", "", "invalid"] {
            assert!(super::new(model).is_err());
        }
        assert!(super::new("tcfilter").is_ok());
    }
}

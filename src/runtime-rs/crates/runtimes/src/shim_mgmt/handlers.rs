// Copyright (c) 2019-2022 Alibaba Cloud
// Copyright (c) 2019-2022 Ant Group
//
// SPDX-License-Identifier: Apache-2.0
//

// This defines the handlers corresponding to the url when a request is sent to destined url,
// the handler function should be invoked, and the corresponding data will be in the response

use crate::shim_metrics::get_shim_metrics;
use anyhow::{anyhow, Context, Result};
use bytes::Bytes;
use common::Sandbox;
use http_body_util::Full;
use hyper::{body::Incoming, Method, Request, Response, StatusCode};
use std::sync::Arc;
use url::Url;

use shim_interface::shim_mgmt::{
    AGENT_POLICY_URL, AGENT_URL, DIRECT_VOLUME_PATH_KEY, DIRECT_VOLUME_RESIZE_URL,
    DIRECT_VOLUME_STATS_URL, METRICS_URL,
};

// main router for response, this works as a multiplexer on
// http arrival which invokes the corresponding handler function
pub(crate) async fn handler_mux(
    sandbox: Arc<dyn Sandbox>,
    req: Request<Incoming>,
) -> Result<Response<Full<Bytes>>> {
    info!(
        sl!(),
        "mgmt-svr(mux): recv req, method: {}, uri: {}",
        req.method(),
        req.uri().path()
    );
    match (req.method(), req.uri().path()) {
        (&Method::GET, AGENT_URL) => agent_url_handler(sandbox, req).await,
        (&Method::POST, DIRECT_VOLUME_STATS_URL) => direct_volume_stats_handler(sandbox, req).await,
        (&Method::POST, DIRECT_VOLUME_RESIZE_URL) => {
            Ok(unsupported("kata-fc: volume resize is unsupported"))
        }
        (&Method::GET, METRICS_URL) => metrics_url_handler(sandbox, req).await,
        (&Method::PUT, AGENT_POLICY_URL) => Ok(unsupported("kata-fc: agent policy is unsupported")),
        _ => Ok(not_found(req).await),
    }
}

// url not found
async fn not_found(_req: Request<Incoming>) -> Response<Full<Bytes>> {
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .body(Full::new(Bytes::from("URL NOT FOUND")))
        .unwrap()
}

// returns the url for agent
async fn agent_url_handler(
    sandbox: Arc<dyn Sandbox>,
    _req: Request<Incoming>,
) -> Result<Response<Full<Bytes>>> {
    let agent_sock = sandbox
        .agent_sock()
        .await
        .unwrap_or_else(|_| String::from(""));
    Ok(Response::new(Full::new(Bytes::from(agent_sock))))
}

async fn direct_volume_stats_handler(
    sandbox: Arc<dyn Sandbox>,
    req: Request<Incoming>,
) -> Result<Response<Full<Bytes>>> {
    let params = Url::parse(&req.uri().to_string())
        .map_err(|e| anyhow!(e))?
        .query_pairs()
        .into_owned()
        .collect::<std::collections::HashMap<String, String>>();
    let volume_path = params
        .get(DIRECT_VOLUME_PATH_KEY)
        .context("shim-mgmt: volume path key not found in request params")?;
    let result = sandbox.direct_volume_stats(volume_path).await;
    match result {
        Ok(stats) => Ok(Response::new(Full::new(Bytes::from(stats)))),
        _ => Err(anyhow!("handler: Failed to get volume stats")),
    }
}

// returns the url for metrics
async fn metrics_url_handler(
    sandbox: Arc<dyn Sandbox>,
    _req: Request<Incoming>,
) -> Result<Response<Full<Bytes>>> {
    // get metrics from agent, hypervisor, and shim
    let agent_metrics = sandbox.agent_metrics().await.unwrap_or_default();
    let hypervisor_metrics = sandbox.hypervisor_metrics().await.unwrap_or_default();
    let shim_metrics = get_shim_metrics().unwrap_or_default();

    Ok(Response::new(Full::new(Bytes::from(format!(
        "{agent_metrics}{hypervisor_metrics}{shim_metrics}"
    )))))
}

fn unsupported(message: &'static str) -> Response<Full<Bytes>> {
    Response::builder()
        .status(StatusCode::NOT_IMPLEMENTED)
        .body(Full::new(Bytes::from(message)))
        .unwrap()
}

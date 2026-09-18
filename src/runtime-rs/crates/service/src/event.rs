// Copyright (c) 2019-2024 Ant Group
//
// SPDX-License-Identifier: Apache-2.0
//

use std::collections::HashMap;
use std::env;
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use common::message::Event;
use containerd_shim::publisher::RemotePublisher;
use containerd_shim::util::timestamp;
use containerd_shim_protos::protobuf::well_known_types::any::Any;
use containerd_shim_protos::shim::event::Envelope;
use containerd_shim_protos::shim::events;
use containerd_shim_protos::shim_async::Events;
use ttrpc::r#async::TtrpcContext;
use ttrpc::MessageHeader;

// Ttrpc address passed from container runtime.
// Required by this containerd-only fork.
const TTRPC_ADDRESS_ENV: &str = "TTRPC_ADDRESS";

/// Containerd event delivery is required; never substitute log-only events.
pub(crate) async fn new_event_publisher(namespace: &str) -> Result<ContainerdForwarder> {
    let address =
        env::var(TTRPC_ADDRESS_ENV).context("kata-fc requires containerd TTRPC_ADDRESS")?;
    anyhow::ensure!(
        !address.is_empty(),
        "kata-fc requires nonempty containerd TTRPC_ADDRESS"
    );
    ContainerdForwarder::new(namespace, &address).await
}

/// Events are forwarded to containerd via ttrpc.
pub(crate) struct ContainerdForwarder {
    namespace: String,
    publisher: RemotePublisher,
}

impl ContainerdForwarder {
    async fn new(namespace: &str, address: &str) -> Result<Self> {
        let publisher = RemotePublisher::new(address)
            .await
            .context("new remote publisher")?;
        Ok(Self {
            namespace: namespace.to_string(),
            publisher,
        })
    }

    fn build_forward_request(
        &self,
        event: &Arc<dyn Event + Send + Sync>,
    ) -> Result<events::ForwardRequest> {
        let mut envelope = Envelope::new();
        envelope.set_topic(event.r#type().clone());
        envelope.set_namespace(self.namespace.to_string());
        envelope.set_timestamp(
            timestamp().map_err(|err| anyhow!("failed to get timestamp: {:?}", err))?,
        );
        envelope.set_event(Any {
            type_url: event.type_url().clone(),
            value: event.value().context("get event value")?,
            ..Default::default()
        });

        let mut req = events::ForwardRequest::new();
        req.set_envelope(envelope);

        Ok(req)
    }
}

impl ContainerdForwarder {
    pub(crate) async fn forward(&self, event: Arc<dyn Event + Send + Sync>) -> Result<()> {
        let req = self
            .build_forward_request(&event)
            .context("build forward request")?;
        self.publisher
            .forward(&default_ttrpc_context(), req)
            .await
            .context("forward")?;
        Ok(())
    }
}

#[inline]
fn default_ttrpc_context() -> TtrpcContext {
    TtrpcContext {
        mh: MessageHeader::default(),
        metadata: HashMap::default(),
        timeout_nano: 0,
    }
}

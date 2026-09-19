// Copyright (c) 2020-2021 Intel Corporation
//
// SPDX-License-Identifier: Apache-2.0
//

use anyhow::Result;
use opentelemetry::sdk::propagation::TraceContextPropagator;
use opentelemetry::trace::TracerProvider;
use opentelemetry::{global, sdk::trace::Config};
use slog::{info, o, Logger};
use std::collections::HashMap;
use tracing_opentelemetry::OpenTelemetryLayer;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::Registry;
use ttrpc::r#async::TtrpcContext;

pub fn setup_tracing(name: &'static str, logger: &Logger) -> Result<()> {
    let logger = logger.new(o!("subsystem" => "vsock-tracer"));

    let exporter = vsock_exporter::Exporter::builder()
        .with_logger(&logger)
        .init();

    let config = Config::default();

    let builder = opentelemetry::sdk::trace::TracerProvider::builder()
        .with_batch_exporter(exporter, opentelemetry::runtime::Tokio)
        .with_config(config);

    let provider = builder.build();

    let tracer = provider.tracer(name);

    let _global_provider = global::set_tracer_provider(provider);

    let layer = OpenTelemetryLayer::new(tracer);

    let subscriber = Registry::default().with(layer);

    tracing::subscriber::set_global_default(subscriber)?;

    global::set_text_map_propagator(TraceContextPropagator::new());

    info!(logger, "tracing setup");

    Ok(())
}

pub fn end_tracing() {
    global::shutdown_tracer_provider();
}

pub fn extract_carrier_from_ttrpc(ttrpc_context: &TtrpcContext) -> HashMap<String, String> {
    let mut carrier = HashMap::new();
    for (k, v) in &ttrpc_context.metadata {
        carrier.insert(k.clone(), v.join(","));
    }

    carrier
}

pub fn set_rpc_parent(ctx: &TtrpcContext) {
    use tracing_opentelemetry::OpenTelemetrySpanExt;
    if !crate::AGENT_CONFIG.tracing {
        return;
    }
    let carrier = extract_carrier_from_ttrpc(ctx);
    let parent = global::get_text_map_propagator(|p| p.extract(&carrier));
    tracing::Span::current().set_parent(parent);
}

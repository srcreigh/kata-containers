// Copyright (c) 2020-2021 Intel Corporation
//
// SPDX-License-Identifier: Apache-2.0
//

// The VSOCK Exporter sends trace spans "out" to the forwarder running on the
// host (which then forwards them on to a trace collector). The data is sent
// via a VSOCK socket that the forwarder process is listening on. To allow the
// forwarder to know how much data to each for each trace span the simplest
// protocol is employed which uses a header packet and the payload (trace
// span) data. The header packet is a simple count of the number of bytes in the
// payload, which allows the forwarder to know how many bytes it must read to
// consume the trace span. The payload is a serialised version of the trace span.

#![allow(unknown_lints)]

use async_trait::async_trait;
use opentelemetry::sdk::export::trace::{ExportResult, SpanData, SpanExporter};
use opentelemetry::sdk::export::ExportError;
use slog::{error, info, o, Logger};
use thiserror::Error;
use tokio::io::AsyncWriteExt;
use tokio_vsock::VsockStream;

// By default, the VSOCK exporter should talk "out" to the host where the
// forwarder is running.
const DEFAULT_CID: u32 = libc::VMADDR_CID_HOST;

// The VSOCK port the forwarders listens on by default
const DEFAULT_PORT: u32 = 10240;

#[derive(Debug)]
pub struct Exporter {
    conn: Option<VsockStream>,
    logger: Logger,
}

impl Exporter {
    pub fn new(logger: &Logger) -> Self {
        Self {
            conn: None,
            logger: logger.new(o!("cid" => DEFAULT_CID, "port" => DEFAULT_PORT)),
        }
    }
}

#[derive(Error, Debug)]
pub enum Error {
    #[error("connection error: {0}")]
    ConnectionError(String),
    #[error("I/O error: {0}")]
    IOError(#[from] std::io::Error),
}

impl ExportError for Error {
    fn exporter_name(&self) -> &'static str {
        "vsock-exporter"
    }
}

// Send a trace span to the forwarder running on the host.
async fn write_span(writer: &mut VsockStream, span: &SpanData) -> std::io::Result<()> {
    let encoded_payload = serde_json::to_vec(span).map_err(std::io::Error::other)?;
    // The forwarder expects an eight-byte network-order payload length.
    writer
        .write_all(&(encoded_payload.len() as u64).to_be_bytes())
        .await?;
    writer.write_all(&encoded_payload).await
}

async fn handle_batch(
    writer: &mut VsockStream,
    batch: Vec<SpanData>,
) -> Result<(), std::io::Error> {
    for span_data in batch {
        write_span(writer, &span_data).await?;
    }

    Ok(())
}

#[async_trait]
impl SpanExporter for Exporter {
    async fn export(&mut self, batch: Vec<SpanData>) -> ExportResult {
        if self.conn.is_none() {
            let conn = connect_vsock().await.map_err(|e| {
                error!(self.logger, "failed to obtain connection"; "error" => format!("{:?}", e));
                e
            })?;

            self.conn = Some(conn);
        }

        handle_batch(self.conn.as_mut().unwrap(), batch)
            .await
            .map_err(|e| {
                error!(self.logger, "handle_batch error: {:?}", e);
                info!(self.logger, "drop failed trace connection");
                self.conn.take();

                Error::IOError(e)
            })?;

        Ok(())
    }

    fn shutdown(&mut self) {
        self.conn.take();
    }
}

async fn connect_vsock() -> Result<VsockStream, Error> {
    match VsockStream::connect(DEFAULT_CID, DEFAULT_PORT).await {
        Ok(conn) => Ok(conn),
        Err(e) => Err(Error::ConnectionError(e.to_string())),
    }
}

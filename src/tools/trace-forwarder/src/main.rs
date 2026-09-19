// Copyright (c) 2020-2021 Intel Corporation
// SPDX-License-Identifier: Apache-2.0
// Restored from Kata's trace forwarder, limited to Firecracker hybrid-vsock.
use anyhow::{ensure, Context, Result};
use clap::Parser;
use opentelemetry::sdk::export::trace::{SpanData, SpanExporter};
use std::io::{ErrorKind, Read};
use std::os::unix::net::UnixListener;

const MAX_SPAN_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Parser)]
struct Args {
    /// Firecracker's master kata.hvsock path; listens on its _10240 sibling.
    #[arg(long)]
    socket_path: String,
    /// Jaeger agent UDP endpoint. Use a collector supporting Jaeger ingestion.
    #[arg(long, default_value = "127.0.0.1:6831")]
    jaeger_endpoint: String,
    #[arg(long, default_value = "kata-agent")]
    trace_name: String,
}

fn read_span(reader: &mut impl Read) -> Result<Option<SpanData>> {
    let mut header = [0u8; 8];
    // EOF is normal only between complete frames.
    match reader.read_exact(&mut header[..1]) {
        Err(e) if e.kind() == ErrorKind::UnexpectedEof => return Ok(None),
        result => result?,
    }
    reader
        .read_exact(&mut header[1..])
        .context("truncated trace header")?;
    let length = u64::from_be_bytes(header);
    ensure!(
        length > 0 && length <= MAX_SPAN_BYTES,
        "invalid trace frame length {length}"
    );
    let mut payload = vec![0; length as usize];
    reader
        .read_exact(&mut payload)
        .context("truncated trace payload")?;
    // Match the guest exporter's JSON format and OpenTelemetry version.
    Ok(Some(
        serde_json::from_slice(&payload).context("invalid trace span")?,
    ))
}

fn main() -> Result<()> {
    let args = Args::parse();
    let path = format!("{}_10240", args.socket_path);
    // Never unlink an existing socket belonging to a running forwarder.
    let listener = UnixListener::bind(&path).with_context(|| format!("bind {path}"))?;
    if nix::unistd::geteuid().is_root() {
        let user = nix::unistd::User::from_name("nobody")?.context("missing nobody user")?;
        nix::unistd::setgroups(&[])?;
        nix::unistd::setgid(user.gid)?;
        nix::unistd::setuid(user.uid)?;
    }
    let mut exporter = opentelemetry_jaeger::new_pipeline()
        .with_service_name(args.trace_name)
        .with_agent_endpoint(args.jaeger_endpoint)
        .init_sync_exporter()?;
    eprintln!("Trace forwarder listening on {path}");
    for connection in listener.incoming() {
        let result = (|| -> Result<()> {
            let mut stream = connection?;
            stream.set_read_timeout(Some(std::time::Duration::from_secs(30)))?;
            while let Some(span) = read_span(&mut stream)? {
                futures::executor::block_on(exporter.export(vec![span]))?;
            }
            Ok(())
        })();
        if let Err(error) = result {
            eprintln!("Trace connection: {error:#}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_invalid_frames_without_panicking() {
        assert!(read_span(&mut &b""[..]).unwrap().is_none());
        assert!(read_span(&mut &b"\0"[..]).is_err());
        for size in [0, MAX_SPAN_BYTES + 1, u64::MAX] {
            assert!(read_span(&mut &size.to_be_bytes()[..]).is_err());
        }
        let mut frame = 3u64.to_be_bytes().to_vec();
        frame.extend_from_slice(b"bad");
        assert!(read_span(&mut frame.as_slice()).is_err());
        frame.pop();
        assert!(read_span(&mut frame.as_slice()).is_err());
    }
}

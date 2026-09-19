// Copyright (c) 2020-2021 Intel Corporation
// SPDX-License-Identifier: Apache-2.0
// Restored from Kata's trace forwarder, limited to Firecracker hybrid-vsock.
use anyhow::{ensure, Context, Result};
use clap::Parser;
use opentelemetry::sdk::export::trace::{SpanData, SpanExporter};
use std::io::{ErrorKind, Read};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixListener;
use std::path::Path;

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

fn bind_socket(path: &str) -> Result<UnixListener> {
    // Never unlink an existing socket belonging to a running forwarder.
    // Full sandbox paths exceed sockaddr_un's 108-byte limit. Bind through
    // an opened parent directory without changing process cwd or unlinking files.
    let socket = Path::new(&path);
    let parent = socket
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let directory = std::fs::File::open(parent)?;
    let short_path = Path::new("/proc/self/fd")
        .join(directory.as_raw_fd().to_string())
        .join(socket.file_name().context("missing socket name")?);
    let listener = UnixListener::bind(short_path).with_context(|| format!("bind {path}"))?;
    drop(directory);
    Ok(listener)
}

fn main() -> Result<()> {
    let args = Args::parse();
    let path = format!("{}_10240", args.socket_path);
    let listener = bind_socket(&path)?;
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
    fn binds_long_sandbox_paths_without_replacing_existing_sockets() {
        let root = tempfile::tempdir().unwrap();
        let parent = root.path().join("a".repeat(100));
        std::fs::create_dir(&parent).unwrap();
        let path = parent.join("kata.hvsock_10240");
        let _listener = bind_socket(path.to_str().unwrap()).unwrap();
        assert!(path.exists());
        assert!(bind_socket(path.to_str().unwrap()).is_err());
    }
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

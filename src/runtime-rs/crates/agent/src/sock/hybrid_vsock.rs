// Copyright (c) 2019-2022 Alibaba Cloud
// Copyright (c) 2019-2022 Ant Group
//
// SPDX-License-Identifier: Apache-2.0
//

use anyhow::{anyhow, Context, Result};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixStream,
};

use super::ConnectConfig;

#[derive(Debug, PartialEq)]
pub struct HybridVsock {
    uds: String,
    port: u32,
}

impl HybridVsock {
    pub fn new(uds: &str, port: u32) -> Self {
        Self {
            uds: uds.to_string(),
            port,
        }
    }
}

impl HybridVsock {
    pub async fn connect(&self, config: &ConnectConfig) -> Result<UnixStream> {
        let mut last_err = None;
        let retry_times = 1 + (config.reconnect_timeout_ms / config.dial_timeout_ms);

        for i in 0..retry_times {
            match tokio::time::timeout(
                std::time::Duration::from_millis(config.dial_timeout_ms),
                connect_helper(&self.uds, self.port),
            )
            .await
            .unwrap_or_else(|_| Err(anyhow!("vsock handshake timed out")))
            {
                Ok(stream) => {
                    info!(sl!(), "hybrid vsock: connected to {:?}", self);
                    return Ok(stream);
                }
                Err(err) => {
                    trace!(
                        sl!(),
                        "hybrid vsock: failed to connect to {:?}, err {:?}, attempts {}, will retry after {} ms",
                        self,
                        err,
                        i,
                        config.dial_timeout_ms,
                    );
                    last_err = Some(err);
                    tokio::time::sleep(std::time::Duration::from_millis(config.dial_timeout_ms))
                        .await;
                    continue;
                }
            }
        }

        // Safe to unwrap the last_err, as this line will be unreachable if
        // no errors occurred.
        Err(anyhow!(
            "hybrid vsock: failed to connect to {:?}, err {:?}",
            self,
            last_err.unwrap()
        ))
    }
}

async fn connect_helper(uds: &str, port: u32) -> Result<UnixStream> {
    let mut stream = UnixStream::connect(&uds).await.context("connect")?;
    stream
        .write_all(format!("connect {port}\n").as_bytes())
        .await
        .context("write all")?;
    // Read exactly the acknowledgement, preserving any coalesced guest payload.
    let mut response = Vec::new();
    loop {
        if response.len() == 32 {
            return Err(anyhow!("oversized vsock acknowledgement"));
        }
        let byte = stream.read_u8().await.context("read acknowledgement")?;
        if byte == b'\n' {
            break;
        }
        response.push(byte);
    }
    let response = std::str::from_utf8(&response).context("invalid acknowledgement")?;
    let port = response
        .strip_prefix("OK ")
        .context("invalid vsock acknowledgement")?;
    if port.is_empty() || !port.bytes().all(|b| b.is_ascii_digit()) || port.parse::<u32>().is_err()
    {
        return Err(anyhow!("invalid vsock acknowledgement port"));
    }
    Ok(stream)
}

#[cfg(test)]
mod security_tests {
    use super::*;
    #[tokio::test]
    async fn exact_handshake_preserves_coalesced_payload_and_rejects_substrings() {
        for (reply, valid) in [
            ("OK 42\npayload", true),
            ("NOT_OK 42\n", false),
            ("OK bad\n", false),
            ("OK 4294967296\n", false),
        ] {
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().join("vsock");
            let listener = tokio::net::UnixListener::bind(&path).unwrap();
            let peer = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                loop {
                    if stream.read_u8().await.unwrap() == b'\n' {
                        break;
                    }
                }
                stream.write_all(reply.as_bytes()).await.unwrap();
            });
            let result = connect_helper(path.to_str().unwrap(), 1024).await;
            if valid {
                let mut tail = String::new();
                result.unwrap().read_to_string(&mut tail).await.unwrap();
                assert_eq!(tail, "payload");
            } else {
                assert!(result.is_err());
            }
            peer.await.unwrap();
        }
    }
}

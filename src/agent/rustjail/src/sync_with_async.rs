// Copyright (c) 2020 Ant Group
//
// SPDX-License-Identifier: Apache-2.0
//

//! The async version of sync module used for IPC

use crate::pipestream::PipeStream;
use anyhow::{anyhow, Result};
use nix::errno::Errno;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::sync::{DATA_SIZE, MSG_SIZE, SYNC_DATA, SYNC_FAILED, SYNC_SUCCESS};

async fn write_count(pipe_w: &mut PipeStream, buf: &[u8]) -> Result<()> {
    let mut len = 0;

    loop {
        match pipe_w.write(&buf[len..]).await {
            Ok(l) => {
                len += l;
                if len == buf.len() {
                    break;
                }
            }

            Err(e) => {
                if e.raw_os_error().unwrap() != Errno::EINTR as i32 {
                    return Err(e.into());
                }
            }
        }
    }

    Ok(())
}

async fn read_count(pipe_r: &mut PipeStream, count: usize) -> Result<Vec<u8>> {
    let mut v: Vec<u8> = vec![0; count];
    let mut len = 0;

    loop {
        match pipe_r.read(&mut v[len..]).await {
            Ok(l) => {
                len += l;
                if len == count || l == 0 {
                    break;
                }
            }

            Err(e) => {
                if e.raw_os_error().unwrap() != Errno::EINTR as i32 {
                    return Err(e.into());
                }
            }
        }
    }

    Ok(v[0..len].to_vec())
}

pub async fn read_async(pipe_r: &mut PipeStream) -> Result<Vec<u8>> {
    let buf = read_count(pipe_r, MSG_SIZE).await?;
    if buf.len() != MSG_SIZE {
        return Err(anyhow!(
            "process: {} failed to receive async message from peer: got msg length: {}, expected: {}",
            std::process::id(),
            buf.len(),
            MSG_SIZE
        ));
    }
    let buf_array: [u8; MSG_SIZE] = [buf[0], buf[1], buf[2], buf[3]];
    let msg: i32 = i32::from_be_bytes(buf_array);
    match msg {
        SYNC_SUCCESS => Ok(Vec::new()),
        SYNC_DATA => {
            let buf = read_count(pipe_r, MSG_SIZE).await?;
            let buf_array: [u8; MSG_SIZE] = [buf[0], buf[1], buf[2], buf[3]];
            let msg_length: i32 = i32::from_be_bytes(buf_array);
            let data_buf = read_count(pipe_r, msg_length as usize).await?;

            Ok(data_buf)
        }
        SYNC_FAILED => {
            let mut error_buf = vec![];
            loop {
                let buf = read_count(pipe_r, DATA_SIZE).await?;

                error_buf.extend(&buf);
                if DATA_SIZE == buf.len() {
                    continue;
                } else {
                    break;
                }
            }

            let error_str = match std::str::from_utf8(&error_buf) {
                Ok(v) => String::from(v),
                Err(e) => {
                    return Err(
                        anyhow!(e).context("receive error message from child process failed")
                    );
                }
            };

            Err(anyhow!(error_str))
        }
        _ => Err(anyhow!("error in receive sync message")),
    }
}

/// Send parent-to-child setup data or an acknowledgement.
pub async fn write_async(pipe_w: &mut PipeStream, msg_type: i32, data_str: &str) -> Result<()> {
    if !matches!(msg_type, SYNC_SUCCESS | SYNC_DATA) {
        return Err(anyhow!("unsupported parent-to-child sync message"));
    }
    write_count(pipe_w, &msg_type.to_be_bytes()).await?;

    if msg_type == SYNC_DATA {
        let length: i32 = data_str.len() as i32;
        write_count(pipe_w, &length.to_be_bytes())
            .await
            .map_err(|e| anyhow!(e).context("error in send message to process"))?;

        write_count(pipe_w, data_str.as_bytes())
            .await
            .map_err(|e| anyhow!(e).context("error in send message to process"))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::{read_sync, write_sync};
    use nix::unistd;
    use std::os::fd::{AsRawFd, IntoRawFd};

    #[tokio::test]
    async fn parent_data_and_acknowledgements_reach_child() {
        let (reader, writer) = unistd::pipe().unwrap();
        let mut writer = PipeStream::new(writer.into_raw_fd()).unwrap();
        for (kind, payload) in [(SYNC_DATA, "setup data"), (SYNC_SUCCESS, "")] {
            write_async(&mut writer, kind, payload).await.unwrap();
            assert_eq!(read_sync(reader.as_raw_fd()).unwrap(), payload.as_bytes());
        }
        assert!(write_async(&mut writer, SYNC_FAILED, "unsupported")
            .await
            .is_err());
    }

    #[tokio::test]
    async fn child_errors_still_reach_parent() {
        let (reader, writer) = unistd::pipe().unwrap();
        let mut reader = PipeStream::new(reader.into_raw_fd()).unwrap();
        // More than one error-read chunk, with EOF terminating the last chunk.
        let error = "child setup failed ".repeat(10);
        write_sync(writer.into_raw_fd(), SYNC_FAILED, &error).unwrap();
        assert_eq!(
            read_async(&mut reader).await.unwrap_err().to_string(),
            error
        );
    }
}

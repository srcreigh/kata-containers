// Copyright (c) 2019 Ant Financial
//
// SPDX-License-Identifier: Apache-2.0
//

use nix::unistd;
use std::mem;
use std::os::fd::BorrowedFd;
use std::os::unix::io::RawFd;

use anyhow::{anyhow, Result};

pub const SYNC_SUCCESS: i32 = 1;
pub const SYNC_FAILED: i32 = 2;
pub const SYNC_DATA: i32 = 3;

pub const DATA_SIZE: usize = 100;
pub const MSG_SIZE: usize = mem::size_of::<i32>();

#[macro_export]
macro_rules! log_child {
    ($fd:expr, $($arg:tt)+) => ({
        let lfd = $fd;
        let mut log_str = format_args!($($arg)+).to_string();
        log_str.push('\n');
        // Ignore error writing to the logger, not much we can do
        let _ = write_count(lfd, log_str.as_bytes());
    })
}

pub fn write_count(fd: RawFd, buf: &[u8]) -> Result<()> {
    let mut len = 0;

    loop {
        let borrowed_fd = unsafe { BorrowedFd::borrow_raw(fd) };
        match unistd::write(borrowed_fd, &buf[len..]) {
            Ok(l) => {
                len += l;
                if len == buf.len() {
                    break;
                }
            }

            Err(e) => {
                if e != nix::Error::EINTR {
                    return Err(e.into());
                }
            }
        }
    }

    Ok(())
}

fn read_count(fd: RawFd, count: usize) -> Result<Vec<u8>> {
    let mut v: Vec<u8> = vec![0; count];
    let mut len = 0;

    loop {
        let borrowed_fd = unsafe { BorrowedFd::borrow_raw(fd) };
        match unistd::read(borrowed_fd, &mut v[len..]) {
            Ok(l) => {
                len += l;
                if len == count || l == 0 {
                    break;
                }
            }

            Err(e) => {
                if e != nix::Error::EINTR {
                    return Err(e.into());
                }
            }
        }
    }

    if len != count {
        Err(anyhow::anyhow!(
            "invalid read count expect {} get {}",
            count,
            len
        ))
    } else {
        Ok(v)
    }
}

pub fn read_sync(fd: RawFd) -> Result<Vec<u8>> {
    let buf = read_count(fd, MSG_SIZE)?;
    let buf_array: [u8; MSG_SIZE] = [buf[0], buf[1], buf[2], buf[3]];
    let msg: i32 = i32::from_be_bytes(buf_array);
    match msg {
        SYNC_SUCCESS => Ok(Vec::new()),
        SYNC_DATA => {
            let buf = read_count(fd, MSG_SIZE)?;
            let buf_array: [u8; MSG_SIZE] = [buf[0], buf[1], buf[2], buf[3]];
            let msg_length: i32 = i32::from_be_bytes(buf_array);
            let data_buf = read_count(fd, msg_length as usize)?;

            Ok(data_buf)
        }
        // The parent sends only data and acknowledgements. Errors flow from
        // this child to read_async in the parent.
        _ => Err(anyhow!("error in receive sync message")),
    }
}

pub fn write_sync(fd: RawFd, msg_type: i32, data_str: &str) -> Result<()> {
    let buf = msg_type.to_be_bytes();

    write_count(fd, &buf)?;

    match msg_type {
        SYNC_FAILED => match write_count(fd, data_str.as_bytes()) {
            Ok(()) => unistd::close(fd)?,
            Err(e) => {
                unistd::close(fd)?;
                return Err(anyhow!(e).context("error in send message to process"));
            }
        },
        SYNC_DATA => {
            let length: i32 = data_str.len() as i32;
            write_count(fd, &length.to_be_bytes()).or_else(|e| {
                unistd::close(fd)?;
                Err(anyhow!(e).context("error in send message to process"))
            })?;

            write_count(fd, data_str.as_bytes()).or_else(|e| {
                unistd::close(fd)?;
                Err(anyhow!(e).context("error in send message to process"))
            })?;
        }

        _ => (),
    };

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::AsRawFd;

    #[test]
    fn child_rejects_parent_error_and_short_header() {
        for bytes in [SYNC_FAILED.to_be_bytes().to_vec(), vec![0, 0]] {
            let (reader, writer) = unistd::pipe().unwrap();
            unistd::write(&writer, &bytes).unwrap();
            drop(writer);
            assert!(read_sync(reader.as_raw_fd()).is_err());
        }
    }
}

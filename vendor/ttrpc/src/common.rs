// Copyright (c) 2020 Ant Financial
//
// SPDX-License-Identifier: Apache-2.0
//

//! Common functions.

use nix::fcntl::{fcntl, FcntlArg, OFlag};
use nix::sys::socket::*;
use std::os::unix::io::RawFd;

use crate::error::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Domain {
    Unix,
    Vsock,
}

pub(crate) fn do_listen(listener: RawFd) -> Result<()> {
    if let Err(e) = fcntl(listener, FcntlArg::F_SETFL(OFlag::O_NONBLOCK)) {
        return Err(Error::Others(format!(
            "failed to set listener fd: {listener} as non block: {e}"
        )));
    }

    listen(listener, 10).map_err(|e| Error::Socket(e.to_string()))
}

fn parse_sockaddr(addr: &str) -> Result<(Domain, &str)> {
    if let Some(addr) = addr.strip_prefix("unix://") {
        return Ok((Domain::Unix, addr));
    }

    if let Some(addr) = addr.strip_prefix("vsock://") {
        return Ok((Domain::Vsock, addr));
    }

    Err(Error::Others(format!("Scheme {addr:?} is not supported")))
}

fn make_addr(sockaddr: &str) -> Result<UnixAddr> {
    if let Some(sockaddr) = sockaddr.strip_prefix('@') {
        UnixAddr::new_abstract(sockaddr.as_bytes()).map_err(err_to_others_err!(e, ""))
    } else {
        UnixAddr::new(sockaddr).map_err(err_to_others_err!(e, ""))
    }
}

// addr: cid:port
// return (cid, port)
fn parse_vscok(addr: &str) -> Result<(u32, u32)> {
    // vsock://cid:port
    let sockaddr_port_v: Vec<&str> = addr.split(':').collect();
    if sockaddr_port_v.len() != 2 {
        return Err(Error::Others(format!(
            "sockaddr {addr} is not right for vsock"
        )));
    }

    // for -1 need trace to libc::VMADDR_CID_ANY
    let cid: u32 = if sockaddr_port_v[0].trim().eq("-1") {
        libc::VMADDR_CID_ANY
    } else {
        sockaddr_port_v[0].parse().map_err(|e| {
            Error::Others(format!(
                "failed to parse cid from {:?} error: {:?}",
                sockaddr_port_v[0], e
            ))
        })?
    };

    let port: u32 = sockaddr_port_v[1].parse().map_err(|e| {
        Error::Others(format!(
            "failed to parse port from {:?} error: {:?}",
            sockaddr_port_v[1], e
        ))
    })?;
    Ok((cid, port))
}

fn make_socket(sockaddr: &str) -> Result<(RawFd, Box<dyn SockaddrLike>)> {
    let (domain, sockaddrv) = parse_sockaddr(sockaddr)?;

    let (fd, sockaddr): (i32, Box<dyn SockaddrLike>) = match domain {
        Domain::Unix => {
            let sockaddr = make_addr(sockaddrv)?;
            let fd = socket(
                AddressFamily::Unix,
                SockType::Stream,
                SockFlag::SOCK_CLOEXEC,
                None,
            )
            .map_err(|e| Error::Socket(e.to_string()))?;
            (fd, Box::new(sockaddr))
        }

        Domain::Vsock => {
            let (cid, port) = parse_vscok(sockaddrv)?;
            let fd = socket(
                AddressFamily::Vsock,
                SockType::Stream,
                SockFlag::SOCK_CLOEXEC,
                None,
            )
            .map_err(|e| Error::Socket(e.to_string()))?;
            let sockaddr = VsockAddr::new(cid, port);
            (fd, Box::new(sockaddr))
        }
    };

    Ok((fd, sockaddr))
}

pub(crate) fn do_bind(sockaddr: &str) -> Result<RawFd> {
    let (fd, sockaddr) = make_socket(sockaddr)?;

    bind(fd, sockaddr.as_ref()).map_err(err_to_others_err!(e, ""))?;

    Ok(fd)
}

/// Creates a unix socket for client.
pub(crate) unsafe fn client_connect(sockaddr: &str) -> Result<RawFd> {
    let (fd, sockaddr) = make_socket(sockaddr)?;

    connect(fd, sockaddr.as_ref())?;

    Ok(fd)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_sockaddr() {
        for i in &[
            (
                "unix:///run/a.sock",
                Some(Domain::Unix),
                "/run/a.sock",
                true,
            ),
            ("vsock://8:1024", Some(Domain::Vsock), "8:1024", true),
            ("Vsock://8:1025", Some(Domain::Vsock), "8:1025", false),
            (
                "unix://@/run/b.sock",
                Some(Domain::Unix),
                "@/run/b.sock",
                true,
            ),
            ("abc:///run/c.sock", None, "", false),
            ("tcp://127.0.0.1:65500", None, "", false),
            ("tcp://[::1]:65500", None, "", false),
            (r"\\.\pipe\ttrpc", None, "", false),
        ] {
            let (input, domain, addr, success) = (i.0, i.1, i.2, i.3);
            let r = parse_sockaddr(input);
            if success {
                let (rd, ra) = r.unwrap();
                assert_eq!(rd, domain.unwrap());
                assert_eq!(ra, addr);
            } else {
                assert!(r.is_err());
            }
        }
    }

    #[test]
    fn test_parse_vscok() {
        for i in &[
            ("-1:1024", (libc::VMADDR_CID_ANY, 1024)),
            ("0:1", (0, 1)),
            ("1:2", (1, 2)),
            ("4294967294:3", (4294967294, 3)),
            // 4294967295 = 0xFFFFFFFF
            ("4294967295:4", (libc::VMADDR_CID_ANY, 4)),
        ] {
            let (input, (cid, port)) = (i.0, i.1);
            let r = parse_vscok(input);
            assert_eq!(r.unwrap(), (cid, port), "parse {:?} failed", i);
        }
    }
}

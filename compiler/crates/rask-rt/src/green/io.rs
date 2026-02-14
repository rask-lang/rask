// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Async I/O wrappers (conc.runtime/IO1-IO3, conc.io-context).
//!
//! Non-blocking I/O futures that register with the reactor when they'd
//! block (EAGAIN). When the FD becomes ready, the reactor wakes the task
//! and the operation retries.

use std::future::Future;
use std::io;
use std::os::unix::io::RawFd;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use super::reactor::{Interest, Reactor};

/// Future that polls a non-blocking read on a raw FD.
///
/// First attempt: try the read immediately (fast path).
/// On EAGAIN: register with reactor, return Pending.
/// On wake: retry the read.
pub struct AsyncRead {
    fd: RawFd,
    reactor: Arc<Reactor>,
    buf_ptr: *mut u8,
    buf_len: usize,
    registered: bool,
}

// Safety: the buffer pointer is valid for the lifetime of the future,
// guaranteed by the caller (ReadGuard pattern or stack pinning).
unsafe impl Send for AsyncRead {}

impl AsyncRead {
    /// Create a new async read future.
    ///
    /// # Safety
    /// The buffer at `buf_ptr` with length `buf_len` must remain valid
    /// until this future completes or is dropped.
    pub unsafe fn new(fd: RawFd, reactor: Arc<Reactor>, buf: &mut [u8]) -> Self {
        Self {
            fd,
            reactor,
            buf_ptr: buf.as_mut_ptr(),
            buf_len: buf.len(),
            registered: false,
        }
    }
}

impl Future for AsyncRead {
    type Output = io::Result<usize>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let n = unsafe {
            libc::read(
                self.fd,
                self.buf_ptr as *mut libc::c_void,
                self.buf_len,
            )
        };

        if n >= 0 {
            // Successful read (including EOF = 0).
            if self.registered {
                let _ = self.reactor.deregister(self.fd);
            }
            return Poll::Ready(Ok(n as usize));
        }

        let err = io::Error::last_os_error();
        if err.kind() == io::ErrorKind::WouldBlock {
            // Register for readability and park.
            if !self.registered {
                self.reactor
                    .register(self.fd, Interest::Readable, cx.waker().clone())
                    .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
                self.registered = true;
            } else {
                // Re-register with fresh waker (task may have migrated workers).
                let _ = self.reactor
                    .register(self.fd, Interest::Readable, cx.waker().clone());
            }
            return Poll::Pending;
        }

        // Real error.
        if self.registered {
            let _ = self.reactor.deregister(self.fd);
        }
        Poll::Ready(Err(err))
    }
}

impl Drop for AsyncRead {
    fn drop(&mut self) {
        if self.registered {
            let _ = self.reactor.deregister(self.fd);
        }
    }
}

/// Future that polls a non-blocking write on a raw FD.
pub struct AsyncWrite {
    fd: RawFd,
    reactor: Arc<Reactor>,
    buf_ptr: *const u8,
    buf_len: usize,
    registered: bool,
}

unsafe impl Send for AsyncWrite {}

impl AsyncWrite {
    /// # Safety
    /// The buffer must remain valid until this future completes or is dropped.
    pub unsafe fn new(fd: RawFd, reactor: Arc<Reactor>, buf: &[u8]) -> Self {
        Self {
            fd,
            reactor,
            buf_ptr: buf.as_ptr(),
            buf_len: buf.len(),
            registered: false,
        }
    }
}

impl Future for AsyncWrite {
    type Output = io::Result<usize>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let n = unsafe {
            libc::write(
                self.fd,
                self.buf_ptr as *const libc::c_void,
                self.buf_len,
            )
        };

        if n >= 0 {
            if self.registered {
                let _ = self.reactor.deregister(self.fd);
            }
            return Poll::Ready(Ok(n as usize));
        }

        let err = io::Error::last_os_error();
        if err.kind() == io::ErrorKind::WouldBlock {
            if !self.registered {
                self.reactor
                    .register(self.fd, Interest::Writable, cx.waker().clone())
                    .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
                self.registered = true;
            } else {
                let _ = self.reactor
                    .register(self.fd, Interest::Writable, cx.waker().clone());
            }
            return Poll::Pending;
        }

        if self.registered {
            let _ = self.reactor.deregister(self.fd);
        }
        Poll::Ready(Err(err))
    }
}

impl Drop for AsyncWrite {
    fn drop(&mut self) {
        if self.registered {
            let _ = self.reactor.deregister(self.fd);
        }
    }
}

/// Future for accepting a connection on a non-blocking listener socket.
pub struct AsyncAccept {
    fd: RawFd,
    reactor: Arc<Reactor>,
    registered: bool,
}

impl AsyncAccept {
    pub fn new(fd: RawFd, reactor: Arc<Reactor>) -> Self {
        Self {
            fd,
            reactor,
            registered: false,
        }
    }
}

impl Future for AsyncAccept {
    type Output = io::Result<(RawFd, std::net::SocketAddr)>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut addr: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
        let mut addrlen = std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;

        let client_fd = unsafe {
            libc::accept4(
                self.fd,
                &mut addr as *mut _ as *mut libc::sockaddr,
                &mut addrlen,
                libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
            )
        };

        if client_fd >= 0 {
            if self.registered {
                let _ = self.reactor.deregister(self.fd);
            }
            // Convert sockaddr to SocketAddr.
            let sock_addr = unsafe { sockaddr_to_std(&addr, addrlen) };
            return Poll::Ready(Ok((client_fd, sock_addr)));
        }

        let err = io::Error::last_os_error();
        if err.kind() == io::ErrorKind::WouldBlock {
            if !self.registered {
                self.reactor
                    .register(self.fd, Interest::Readable, cx.waker().clone())
                    .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
                self.registered = true;
            } else {
                let _ = self.reactor
                    .register(self.fd, Interest::Readable, cx.waker().clone());
            }
            return Poll::Pending;
        }

        if self.registered {
            let _ = self.reactor.deregister(self.fd);
        }
        Poll::Ready(Err(err))
    }
}

impl Drop for AsyncAccept {
    fn drop(&mut self) {
        if self.registered {
            let _ = self.reactor.deregister(self.fd);
        }
    }
}

/// Convert a raw sockaddr_storage to std::net::SocketAddr.
unsafe fn sockaddr_to_std(
    addr: &libc::sockaddr_storage,
    _len: libc::socklen_t,
) -> std::net::SocketAddr {
    match addr.ss_family as i32 {
        libc::AF_INET => {
            let addr4 = &*(addr as *const _ as *const libc::sockaddr_in);
            let ip = std::net::Ipv4Addr::from(u32::from_be(addr4.sin_addr.s_addr));
            let port = u16::from_be(addr4.sin_port);
            std::net::SocketAddr::V4(std::net::SocketAddrV4::new(ip, port))
        }
        libc::AF_INET6 => {
            let addr6 = &*(addr as *const _ as *const libc::sockaddr_in6);
            let ip = std::net::Ipv6Addr::from(addr6.sin6_addr.s6_addr);
            let port = u16::from_be(addr6.sin6_port);
            std::net::SocketAddr::V6(std::net::SocketAddrV6::new(
                ip,
                port,
                addr6.sin6_flowinfo,
                addr6.sin6_scope_id,
            ))
        }
        _ => std::net::SocketAddr::V4(std::net::SocketAddrV4::new(
            std::net::Ipv4Addr::UNSPECIFIED,
            0,
        )),
    }
}

/// Set a file descriptor to non-blocking mode.
pub fn set_nonblocking(fd: RawFd) -> io::Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    let ret = unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) };
    if ret < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

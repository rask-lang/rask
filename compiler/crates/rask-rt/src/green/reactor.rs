// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Reactor: epoll-based I/O event loop (conc.runtime/R1-R3).
//!
//! Central reactor with dedicated thread. Level-triggered epoll.
//! Tasks register interest in FDs; reactor wakes them when ready.

use std::collections::HashMap;
use std::io;
use std::os::unix::io::RawFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::task::Waker;

/// I/O interest for reactor registration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Interest {
    Readable,
    Writable,
    ReadWrite,
}

impl Interest {
    fn to_epoll_events(self) -> u32 {
        match self {
            Interest::Readable => libc::EPOLLIN as u32,
            Interest::Writable => libc::EPOLLOUT as u32,
            Interest::ReadWrite => (libc::EPOLLIN | libc::EPOLLOUT) as u32,
        }
    }
}

/// Per-FD registration: waker + interest.
#[allow(dead_code)]
struct Registration {
    waker: Waker,
    interest: Interest,
}

/// Central I/O reactor backed by epoll (Linux).
///
/// Single reactor thread polls for I/O readiness and wakes parked tasks.
/// Level-triggered: spurious wakeups are harmless (task re-registers).
pub struct Reactor {
    epoll_fd: RawFd,
    /// Eventfd for waking the reactor thread (e.g. new registrations, shutdown).
    wake_fd: RawFd,
    /// FD → registration mapping.
    registrations: Mutex<HashMap<RawFd, Registration>>,
    /// Shutdown flag.
    shutdown: AtomicBool,
}

impl Reactor {
    /// Create a new reactor with an epoll instance and wake eventfd.
    pub fn new() -> io::Result<Self> {
        let epoll_fd = unsafe { libc::epoll_create1(libc::EPOLL_CLOEXEC) };
        if epoll_fd < 0 {
            return Err(io::Error::last_os_error());
        }

        let wake_fd = unsafe { libc::eventfd(0, libc::EFD_NONBLOCK | libc::EFD_CLOEXEC) };
        if wake_fd < 0 {
            unsafe { libc::close(epoll_fd) };
            return Err(io::Error::last_os_error());
        }

        // Register wake_fd with epoll so we can interrupt epoll_wait.
        let mut ev = libc::epoll_event {
            events: libc::EPOLLIN as u32,
            u64: wake_fd as u64,
        };
        let ret =
            unsafe { libc::epoll_ctl(epoll_fd, libc::EPOLL_CTL_ADD, wake_fd, &mut ev) };
        if ret < 0 {
            unsafe {
                libc::close(wake_fd);
                libc::close(epoll_fd);
            }
            return Err(io::Error::last_os_error());
        }

        Ok(Self {
            epoll_fd,
            wake_fd,
            registrations: Mutex::new(HashMap::new()),
            shutdown: AtomicBool::new(false),
        })
    }

    /// Register a file descriptor with the reactor.
    ///
    /// When the FD becomes ready for the given interest, the waker is called,
    /// which re-enqueues the associated task with the scheduler.
    pub fn register(&self, fd: RawFd, interest: Interest, waker: Waker) -> io::Result<()> {
        let mut regs = self.registrations.lock().unwrap();

        let mut ev = libc::epoll_event {
            events: interest.to_epoll_events(),
            u64: fd as u64,
        };

        let op = if regs.contains_key(&fd) {
            libc::EPOLL_CTL_MOD
        } else {
            libc::EPOLL_CTL_ADD
        };

        let ret = unsafe { libc::epoll_ctl(self.epoll_fd, op, fd, &mut ev) };
        if ret < 0 {
            return Err(io::Error::last_os_error());
        }

        regs.insert(fd, Registration { waker, interest });

        // Wake reactor thread so it picks up the new registration.
        self.wake();
        Ok(())
    }

    /// Remove a file descriptor from the reactor.
    pub fn deregister(&self, fd: RawFd) -> io::Result<()> {
        let mut regs = self.registrations.lock().unwrap();
        if regs.remove(&fd).is_some() {
            let ret = unsafe {
                libc::epoll_ctl(
                    self.epoll_fd,
                    libc::EPOLL_CTL_DEL,
                    fd,
                    std::ptr::null_mut(),
                )
            };
            if ret < 0 {
                let err = io::Error::last_os_error();
                // ENOENT / EBADF are expected if FD was already closed.
                if err.raw_os_error() != Some(libc::ENOENT)
                    && err.raw_os_error() != Some(libc::EBADF)
                {
                    return Err(err);
                }
            }
        }
        Ok(())
    }

    /// Run one poll cycle. Called by the reactor thread.
    ///
    /// Blocks up to `timeout_ms` waiting for events. Wakes tasks whose
    /// FDs are ready. Returns the number of tasks woken.
    pub fn poll_once(&self, timeout_ms: i32) -> io::Result<usize> {
        const MAX_EVENTS: usize = 64;
        let mut events: [libc::epoll_event; MAX_EVENTS] =
            [libc::epoll_event { events: 0, u64: 0 }; MAX_EVENTS];

        let n = unsafe {
            libc::epoll_wait(self.epoll_fd, events.as_mut_ptr(), MAX_EVENTS as i32, timeout_ms)
        };

        if n < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                return Ok(0); // EINTR: retry next cycle.
            }
            return Err(err);
        }

        // Collect wakers under the lock, then wake outside it.
        // Wakers may acquire other locks (schedule_fn, work_available);
        // holding registrations during wake() risks deadlock.
        let mut to_wake = Vec::new();

        {
            let regs = self.registrations.lock().unwrap();

            for i in 0..n as usize {
                let fd = events[i].u64 as RawFd;

                // Drain wake_fd reads (just a signal, value doesn't matter).
                if fd == self.wake_fd {
                    let mut buf = [0u8; 8];
                    unsafe {
                        libc::read(self.wake_fd, buf.as_mut_ptr() as *mut libc::c_void, 8);
                    }
                    continue;
                }

                if let Some(reg) = regs.get(&fd) {
                    to_wake.push(reg.waker.clone());
                }
            }
        } // registrations lock released

        let woken = to_wake.len();
        for waker in to_wake {
            waker.wake();
        }

        Ok(woken)
    }

    /// Signal the reactor thread to wake up (e.g. for new registrations
    /// or shutdown).
    pub fn wake(&self) {
        let val: u64 = 1;
        unsafe {
            libc::write(
                self.wake_fd,
                &val as *const u64 as *const libc::c_void,
                8,
            );
        }
    }

    /// Request shutdown. The reactor loop checks this each cycle.
    pub fn request_shutdown(&self) {
        self.shutdown.store(true, Ordering::Release);
        self.wake();
    }

    pub fn should_shutdown(&self) -> bool {
        self.shutdown.load(Ordering::Acquire)
    }
}

impl Drop for Reactor {
    fn drop(&mut self) {
        // Deregister all FDs, then close epoll + eventfd.
        let regs = self.registrations.lock().unwrap();
        for &fd in regs.keys() {
            unsafe {
                libc::epoll_ctl(
                    self.epoll_fd,
                    libc::EPOLL_CTL_DEL,
                    fd,
                    std::ptr::null_mut(),
                );
            }
        }
        drop(regs);

        unsafe {
            libc::close(self.wake_fd);
            libc::close(self.epoll_fd);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::task::{Wake, Waker};

    struct TestWaker {
        woken: AtomicBool,
    }

    impl Wake for TestWaker {
        fn wake(self: Arc<Self>) {
            self.woken.store(true, Ordering::Release);
        }
    }

    #[test]
    fn reactor_lifecycle() {
        let reactor = Reactor::new().unwrap();
        reactor.request_shutdown();
        assert!(reactor.should_shutdown());
    }

    #[test]
    fn reactor_pipe_readiness() {
        let reactor = Reactor::new().unwrap();

        // Create a pipe: write end → read end.
        let mut fds = [0i32; 2];
        unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_NONBLOCK | libc::O_CLOEXEC) };
        let (read_fd, write_fd) = (fds[0], fds[1]);

        let tw = Arc::new(TestWaker {
            woken: AtomicBool::new(false),
        });
        let waker = Waker::from(tw.clone());

        reactor
            .register(read_fd, Interest::Readable, waker)
            .unwrap();

        // Write something to make the read end readable.
        unsafe {
            libc::write(write_fd, b"x".as_ptr() as *const libc::c_void, 1);
        }

        let woken = reactor.poll_once(100).unwrap();
        assert!(woken > 0);
        assert!(tw.woken.load(Ordering::Acquire));

        // Cleanup.
        reactor.deregister(read_fd).unwrap();
        unsafe {
            libc::close(read_fd);
            libc::close(write_fd);
        }
    }

    #[test]
    fn reactor_timeout_no_events() {
        let reactor = Reactor::new().unwrap();
        // Poll with short timeout — no FDs registered, should return 0.
        let woken = reactor.poll_once(1).unwrap();
        assert_eq!(woken, 0);
    }
}

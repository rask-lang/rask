// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Channels: a queue under a mutex, and a condvar every change is announced on.
//!
//! `std::sync::mpsc` served until a cancel had to wake a task parked in a
//! receive (conc.async/CN3): nothing outside an mpsc end can wake it, so the
//! waits polled. Here a wait is a condvar wait a cancel can notify, and a
//! `select` sleeps on one epoch every channel moves, the way the native
//! runtime's does.
//!
//! The ends are counted. A channel is closed for receivers once every sender
//! is gone and the buffer is drained, and for senders once every receiver is.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

use crate::value::{CancelToken, Value};

pub struct Chan {
    state: Mutex<State>,
    changed: Condvar,
}

struct State {
    buf: VecDeque<Value>,
    /// 0 is a rendezvous: a sender offers one value and waits for it taken.
    cap: usize,
    senders: usize,
    receivers: usize,
    /// Rendezvous only: the value a sender is offering, and whether a
    /// receiver has taken it.
    offer: Option<Value>,
    offering: bool,
    taken: bool,
    /// The offer's sender waits to see it taken. A `try_send` offer has
    /// nobody waiting, so taking it frees the slot.
    offer_waits: bool,
    /// Receivers parked in `recv`, so a rendezvous `try_send` knows one is
    /// there to take it.
    waiting_receivers: usize,
}

pub enum SendError {
    Closed(Value),
    Full(Value),
    Cancelled(Value),
}

pub enum RecvError {
    Closed,
    Empty,
    Cancelled,
}

impl Chan {
    pub fn pair(cap: usize) -> (Arc<SenderEnd>, Arc<ReceiverEnd>) {
        let chan = Arc::new(Chan {
            state: Mutex::new(State {
                buf: VecDeque::new(),
                cap,
                senders: 1,
                receivers: 1,
                offer: None,
                offering: false,
                taken: false,
                offer_waits: false,
                waiting_receivers: 0,
            }),
            changed: Condvar::new(),
        });
        (SenderEnd::new(chan.clone()), ReceiverEnd::new(chan))
    }

    fn announce(&self) {
        self.changed.notify_all();
        select_epoch_bump();
    }

    /// Wait on the channel, woken by a change or by the task's cancel.
    fn wait<'a>(&self, guard: MutexGuard<'a, State>) -> MutexGuard<'a, State> {
        self.changed.wait(guard).unwrap()
    }

    pub fn send(self: &Arc<Self>, value: Value) -> Result<(), SendError> {
        let token = crate::value::current_cancel();
        let _wake = token.as_ref().map(|t| t.wake_on_cancel(self.waker()));
        let cancelled = || token.as_ref().is_some_and(|t| t.is_cancelled());
        let mut st = self.state.lock().unwrap();
        if st.cap > 0 {
            loop {
                if st.receivers == 0 {
                    return Err(SendError::Closed(value));
                }
                if st.buf.len() < st.cap {
                    st.buf.push_back(value);
                    self.announce();
                    return Ok(());
                }
                if cancelled() {
                    return Err(SendError::Cancelled(value));
                }
                st = self.wait(st);
            }
        }
        // Rendezvous: wait for the slot, offer, wait for it taken.
        while st.offering {
            if st.receivers == 0 {
                return Err(SendError::Closed(value));
            }
            if cancelled() {
                return Err(SendError::Cancelled(value));
            }
            st = self.wait(st);
        }
        if st.receivers == 0 {
            return Err(SendError::Closed(value));
        }
        st.offer = Some(value);
        st.offering = true;
        st.taken = false;
        st.offer_waits = true;
        self.announce();
        let mut withdrawn = None;
        while !st.taken {
            if st.receivers == 0 {
                withdrawn = st.offer.take().map(SendError::Closed);
                break;
            }
            if cancelled() {
                withdrawn = st.offer.take().map(SendError::Cancelled);
                break;
            }
            st = self.wait(st);
        }
        st.offering = false;
        st.taken = false;
        self.announce();
        match withdrawn {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    pub fn try_send(&self, value: Value) -> Result<(), SendError> {
        let mut st = self.state.lock().unwrap();
        if st.receivers == 0 {
            return Err(SendError::Closed(value));
        }
        if st.cap > 0 {
            if st.buf.len() >= st.cap {
                return Err(SendError::Full(value));
            }
            st.buf.push_back(value);
            self.announce();
            return Ok(());
        }
        // A rendezvous only goes through with a receiver already waiting for
        // it. The offer stays up for that receiver; nobody waits for it taken.
        if st.offering || st.waiting_receivers == 0 {
            return Err(SendError::Full(value));
        }
        st.offer = Some(value);
        st.offering = true;
        st.taken = false;
        st.offer_waits = false;
        self.announce();
        Ok(())
    }

    pub fn recv(self: &Arc<Self>) -> Result<Value, RecvError> {
        let token = crate::value::current_cancel();
        let _wake = token.as_ref().map(|t| t.wake_on_cancel(self.waker()));
        let mut st = self.state.lock().unwrap();
        loop {
            // A value that is there is taken, cancel or not.
            if let Some(v) = Self::take(&mut st) {
                self.announce();
                return Ok(v);
            }
            if st.senders == 0 {
                return Err(RecvError::Closed);
            }
            if token.as_ref().is_some_and(|t| t.is_cancelled()) {
                return Err(RecvError::Cancelled);
            }
            st.waiting_receivers += 1;
            st = self.wait(st);
            st.waiting_receivers -= 1;
        }
    }

    pub fn try_recv(&self) -> Result<Value, RecvError> {
        let mut st = self.state.lock().unwrap();
        if let Some(v) = Self::take(&mut st) {
            self.announce();
            return Ok(v);
        }
        if st.senders == 0 {
            return Err(RecvError::Closed);
        }
        Err(RecvError::Empty)
    }

    fn take(st: &mut State) -> Option<Value> {
        if st.cap > 0 {
            return st.buf.pop_front();
        }
        if st.offering && !st.taken {
            let v = st.offer.take()?;
            if st.offer_waits {
                st.taken = true;
            } else {
                st.offering = false;
            }
            return Some(v);
        }
        None
    }

    fn waker(self: &Arc<Self>) -> Arc<dyn Fn() + Send + Sync> {
        let chan = self.clone();
        Arc::new(move || {
            let _held = chan.state.lock().unwrap();
            chan.changed.notify_all();
        })
    }

    fn end_gone(&self, sender: bool) {
        let mut st = self.state.lock().unwrap();
        if sender {
            st.senders -= 1;
        } else {
            st.receivers -= 1;
        }
        drop(st);
        self.announce();
    }
}

/// One sender. Closing it, or dropping the last value that holds it, closes
/// this end; the channel is closed for receivers when every sender is.
pub struct SenderEnd {
    chan: Arc<Chan>,
    open: AtomicBool,
}

impl SenderEnd {
    fn new(chan: Arc<Chan>) -> Arc<Self> {
        Arc::new(SenderEnd { chan, open: AtomicBool::new(true) })
    }

    pub fn send(&self, value: Value) -> Result<(), SendError> {
        if !self.open.load(Ordering::Acquire) {
            return Err(SendError::Closed(value));
        }
        self.chan.send(value)
    }

    pub fn try_send(&self, value: Value) -> Result<(), SendError> {
        if !self.open.load(Ordering::Acquire) {
            return Err(SendError::Closed(value));
        }
        self.chan.try_send(value)
    }

    /// Another sender on the same channel.
    pub fn clone_end(&self) -> Arc<SenderEnd> {
        self.chan.state.lock().unwrap().senders += 1;
        SenderEnd::new(self.chan.clone())
    }

    pub fn close(&self) {
        if self.open.swap(false, Ordering::AcqRel) {
            self.chan.end_gone(true);
        }
    }
}

impl std::fmt::Debug for SenderEnd {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SenderEnd")
    }
}

impl Drop for SenderEnd {
    fn drop(&mut self) {
        self.close();
    }
}

pub struct ReceiverEnd {
    chan: Arc<Chan>,
    open: AtomicBool,
}

impl ReceiverEnd {
    fn new(chan: Arc<Chan>) -> Arc<Self> {
        Arc::new(ReceiverEnd { chan, open: AtomicBool::new(true) })
    }

    pub fn recv(&self) -> Result<Value, RecvError> {
        if !self.open.load(Ordering::Acquire) {
            return Err(RecvError::Closed);
        }
        self.chan.recv()
    }

    pub fn try_recv(&self) -> Result<Value, RecvError> {
        if !self.open.load(Ordering::Acquire) {
            return Err(RecvError::Closed);
        }
        self.chan.try_recv()
    }

    pub fn close(&self) {
        if self.open.swap(false, Ordering::AcqRel) {
            self.chan.end_gone(false);
        }
    }
}

impl std::fmt::Debug for ReceiverEnd {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ReceiverEnd")
    }
}

impl Drop for ReceiverEnd {
    fn drop(&mut self) {
        self.close();
    }
}

// ─── Select ────────────────────────────────────────────────
//
// A `select` with nothing ready waits for *any* of its channels to change, and
// one wait can't sit on several condvars. So every change to any channel moves
// one epoch, and a waiting select sleeps until it moves past what it read
// before probing its arms. Channel operations skip the lock while no select
// waits.

static SELECT_EPOCH: AtomicU64 = AtomicU64::new(0);
static SELECT_LOCK: Mutex<()> = Mutex::new(());
static SELECT_MOVED: Condvar = Condvar::new();
static SELECT_WAITERS: AtomicUsize = AtomicUsize::new(0);

fn select_epoch_bump() {
    SELECT_EPOCH.fetch_add(1, Ordering::SeqCst);
    // A select counts itself in before it looks at the epoch, so one that
    // isn't counted yet will see this move.
    if SELECT_WAITERS.load(Ordering::SeqCst) > 0 {
        let _held = SELECT_LOCK.lock().unwrap();
        SELECT_MOVED.notify_all();
    }
}

pub fn select_epoch() -> u64 {
    SELECT_EPOCH.load(Ordering::SeqCst)
}

/// Sleep until some channel changes after `seen`, or the task is cancelled.
/// True when cancelled.
pub fn select_wait(seen: u64, token: Option<&Arc<CancelToken>>) -> bool {
    let _wake = token.map(|t| {
        t.wake_on_cancel(Arc::new(|| {
            let _held = SELECT_LOCK.lock().unwrap();
            SELECT_MOVED.notify_all();
        }))
    });
    let cancelled = || token.is_some_and(|t| t.is_cancelled());
    SELECT_WAITERS.fetch_add(1, Ordering::SeqCst);
    let mut held = SELECT_LOCK.lock().unwrap();
    while SELECT_EPOCH.load(Ordering::SeqCst) == seen && !cancelled() {
        held = SELECT_MOVED.wait(held).unwrap();
    }
    drop(held);
    SELECT_WAITERS.fetch_sub(1, Ordering::SeqCst);
    cancelled()
}

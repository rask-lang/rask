// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Channels (conc.async/CH1-CH4).
//!
//! Phase A: wraps `std::sync::mpsc`. Blocking send/recv.

use std::sync::mpsc;

/// Errors from channel operations.
#[derive(Debug)]
pub enum SendError<T> {
    /// All receivers dropped.
    Closed(T),
}

#[derive(Debug)]
pub enum RecvError {
    /// All senders dropped and buffer empty.
    Closed,
}

#[derive(Debug)]
pub enum TrySendError<T> {
    /// Buffer is full.
    Full(T),
    /// All receivers dropped.
    Closed(T),
}

#[derive(Debug)]
pub enum TryRecvError {
    /// No message available right now.
    Empty,
    /// All senders dropped.
    Closed,
}

/// Create a buffered channel with capacity `n` (CH2).
pub fn buffered<T>(n: usize) -> (Sender<T>, Receiver<T>) {
    let (tx, rx) = mpsc::sync_channel(n);
    (Sender { inner: tx }, Receiver { inner: rx })
}

/// Create an unbuffered (rendezvous) channel (CH2).
pub fn unbuffered<T>() -> (Sender<T>, Receiver<T>) {
    let (tx, rx) = mpsc::sync_channel(0);
    (Sender { inner: tx }, Receiver { inner: rx })
}

/// Sending half of a channel (CH1: non-linear, can be dropped).
pub struct Sender<T> {
    pub(crate) inner: mpsc::SyncSender<T>,
}

impl<T> Sender<T> {
    /// Blocking send. Pauses/blocks if buffer full.
    pub fn send(&self, val: T) -> Result<(), SendError<T>> {
        self.inner.send(val).map_err(|e| SendError::Closed(e.0))
    }

    /// Non-blocking send attempt.
    pub fn try_send(&self, val: T) -> Result<(), TrySendError<T>> {
        match self.inner.try_send(val) {
            Ok(()) => Ok(()),
            Err(mpsc::TrySendError::Full(v)) => Err(TrySendError::Full(v)),
            Err(mpsc::TrySendError::Disconnected(v)) => Err(TrySendError::Closed(v)),
        }
    }
}

// Sender is Clone (multiple producers)
impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        Sender {
            inner: self.inner.clone(),
        }
    }
}

/// Receiving half of a channel (CH1: non-linear, can be dropped).
pub struct Receiver<T> {
    pub(crate) inner: mpsc::Receiver<T>,
}

impl<T> Receiver<T> {
    /// Blocking receive. Pauses/blocks if buffer empty.
    pub fn recv(&self) -> Result<T, RecvError> {
        self.inner.recv().map_err(|_| RecvError::Closed)
    }

    /// Non-blocking receive attempt.
    pub fn try_recv(&self) -> Result<T, TryRecvError> {
        match self.inner.try_recv() {
            Ok(val) => Ok(val),
            Err(mpsc::TryRecvError::Empty) => Err(TryRecvError::Empty),
            Err(mpsc::TryRecvError::Disconnected) => Err(TryRecvError::Closed),
        }
    }

    /// Receive with timeout.
    pub fn recv_timeout(&self, timeout: std::time::Duration) -> Result<T, TryRecvError> {
        match self.inner.recv_timeout(timeout) {
            Ok(val) => Ok(val),
            Err(mpsc::RecvTimeoutError::Timeout) => Err(TryRecvError::Empty),
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(TryRecvError::Closed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffered_send_recv() {
        let (tx, rx) = buffered(10);
        tx.send(42).unwrap();
        assert_eq!(rx.recv().unwrap(), 42);
    }

    #[test]
    fn unbuffered_rendezvous() {
        let (tx, rx) = unbuffered();
        std::thread::spawn(move || {
            tx.send(99).unwrap();
        });
        assert_eq!(rx.recv().unwrap(), 99);
    }

    #[test]
    fn closed_channel() {
        let (tx, rx) = buffered::<i32>(10);
        drop(tx);
        assert!(matches!(rx.recv(), Err(RecvError::Closed)));
    }

    #[test]
    fn try_recv_empty() {
        let (_tx, rx) = buffered::<i32>(10);
        assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
    }

    #[test]
    fn multiple_producers() {
        let (tx, rx) = buffered(10);
        let tx2 = tx.clone();
        std::thread::spawn(move || tx.send(1).unwrap());
        std::thread::spawn(move || tx2.send(2).unwrap());
        let mut vals = vec![rx.recv().unwrap(), rx.recv().unwrap()];
        vals.sort();
        assert_eq!(vals, vec![1, 2]);
    }
}

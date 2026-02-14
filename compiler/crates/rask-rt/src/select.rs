// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Select (conc.select/A1-A3, P1-P2).
//!
//! Phase A: poll channels with try_recv in a spin loop with backoff.
//! Phase B: reactor-integrated wait with epoll/kqueue.

use std::time::Duration;

use crate::channel::{Receiver, TryRecvError};

/// Result of a select operation.
#[derive(Debug)]
pub enum SelectResult<T> {
    /// Received a value from channel at the given index.
    Recv(usize, T),
    /// Default arm fired (non-blocking, all channels empty).
    Default,
    /// All channels closed.
    AllClosed,
}

/// Polling-based select over multiple receivers (P1: random fair).
///
/// Phase A implementation: try_recv each channel in random order.
/// Spins with exponential backoff up to 1ms between polls.
///
/// `has_default`: if true, returns `Default` immediately when all empty.
pub fn select_recv<T>(receivers: &[&Receiver<T>], has_default: bool) -> SelectResult<T> {
    if receivers.is_empty() {
        return SelectResult::AllClosed;
    }

    // Build permuted index array for fair selection (P1)
    let mut indices: Vec<usize> = (0..receivers.len()).collect();

    // Simple random permutation using pointer address as seed
    let seed = &indices as *const _ as u64;
    let mut rng = seed.wrapping_add(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos() as u64,
    );
    for i in (1..indices.len()).rev() {
        rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
        let j = (rng as usize) % (i + 1);
        indices.swap(i, j);
    }

    let mut backoff = Duration::from_nanos(100);
    let max_backoff = Duration::from_millis(1);

    loop {
        let mut all_closed = true;

        for &idx in &indices {
            match receivers[idx].try_recv() {
                Ok(val) => return SelectResult::Recv(idx, val),
                Err(TryRecvError::Empty) => {
                    all_closed = false;
                }
                Err(TryRecvError::Closed) => {
                    // This channel is closed, skip it
                }
            }
        }

        if all_closed {
            return SelectResult::AllClosed;
        }

        if has_default {
            return SelectResult::Default;
        }

        // Backoff before next poll
        std::thread::sleep(backoff);
        backoff = (backoff * 2).min(max_backoff);
    }
}

/// Priority select: evaluates arms in listed order (P2).
pub fn select_priority_recv<T>(receivers: &[&Receiver<T>], has_default: bool) -> SelectResult<T> {
    if receivers.is_empty() {
        return SelectResult::AllClosed;
    }

    let mut backoff = Duration::from_nanos(100);
    let max_backoff = Duration::from_millis(1);

    loop {
        let mut all_closed = true;

        for (idx, rx) in receivers.iter().enumerate() {
            match rx.try_recv() {
                Ok(val) => return SelectResult::Recv(idx, val),
                Err(TryRecvError::Empty) => {
                    all_closed = false;
                }
                Err(TryRecvError::Closed) => {}
            }
        }

        if all_closed {
            return SelectResult::AllClosed;
        }

        if has_default {
            return SelectResult::Default;
        }

        std::thread::sleep(backoff);
        backoff = (backoff * 2).min(max_backoff);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channel;

    #[test]
    fn select_single_channel() {
        let (tx, rx) = channel::buffered(10);
        tx.send(42).unwrap();
        match select_recv(&[&rx], false) {
            SelectResult::Recv(0, val) => assert_eq!(val, 42),
            other => panic!("expected Recv(0, 42), got {:?}", other),
        }
    }

    #[test]
    fn select_multiple_channels() {
        let (tx1, rx1) = channel::buffered(10);
        let (_tx2, rx2) = channel::buffered::<i32>(10);
        tx1.send(99).unwrap();
        match select_recv(&[&rx1, &rx2], false) {
            SelectResult::Recv(0, val) => assert_eq!(val, 99),
            other => panic!("expected Recv(0, 99), got {:?}", other),
        }
    }

    #[test]
    fn select_default_arm() {
        let (_tx, rx) = channel::buffered::<i32>(10);
        match select_recv(&[&rx], true) {
            SelectResult::Default => {}
            other => panic!("expected Default, got {:?}", other),
        }
    }

    #[test]
    fn select_all_closed() {
        let (tx, rx) = channel::buffered::<i32>(10);
        drop(tx);
        match select_recv(&[&rx], false) {
            SelectResult::AllClosed => {}
            other => panic!("expected AllClosed, got {:?}", other),
        }
    }

    #[test]
    fn select_priority_first_ready_wins() {
        let (tx1, rx1) = channel::buffered(10);
        let (tx2, rx2) = channel::buffered(10);
        tx1.send(1).unwrap();
        tx2.send(2).unwrap();
        // Priority: rx1 always checked first
        match select_priority_recv(&[&rx1, &rx2], false) {
            SelectResult::Recv(0, val) => assert_eq!(val, 1),
            other => panic!("expected Recv(0, 1), got {:?}", other),
        }
    }
}

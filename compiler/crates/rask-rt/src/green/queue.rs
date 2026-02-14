// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Work-stealing task queues (conc.runtime/S1, S3).
//!
//! Per-worker bounded FIFO + global injection queue. Workers steal from
//! each other when idle.
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use super::task::RawTask;

/// Per-worker local queue. Mutex-protected VecDeque.
///
/// Owner pushes/pops from the front, stealers take from the back.
/// Both paths go through the same mutex — acceptable for initial
/// implementation but should migrate to a lock-free Chase-Lev deque
/// if profiling shows contention.
///
/// Bounded to `CAPACITY` entries. Overflow goes to the global queue.
pub(crate) struct LocalQueue {
    deque: Mutex<VecDeque<Arc<RawTask>>>,
}

/// Max tasks in a single worker's local queue before overflow.
const CAPACITY: usize = 1024;

#[allow(dead_code)]
impl LocalQueue {
    pub fn new() -> Self {
        Self {
            deque: Mutex::new(VecDeque::with_capacity(CAPACITY)),
        }
    }

    /// Push a task. Returns Err if the queue is full.
    pub fn push(&self, task: Arc<RawTask>) -> Result<(), Arc<RawTask>> {
        let mut q = self.deque.lock().unwrap();
        if q.len() >= CAPACITY {
            return Err(task);
        }
        q.push_back(task);
        Ok(())
    }

    /// Pop from the front (owner's fast path).
    pub fn pop(&self) -> Option<Arc<RawTask>> {
        self.deque.lock().unwrap().pop_front()
    }

    /// Steal half the queue from the back (other workers call this).
    /// Returns stolen tasks (may be empty).
    pub fn steal_batch(&self) -> Vec<Arc<RawTask>> {
        let mut q = self.deque.lock().unwrap();
        let count = q.len() / 2;
        if count == 0 {
            return if q.is_empty() {
                Vec::new()
            } else {
                // Steal at least one if there's work.
                q.pop_back().into_iter().collect()
            };
        }
        let mut stolen = Vec::with_capacity(count);
        for _ in 0..count {
            if let Some(task) = q.pop_back() {
                stolen.push(task);
            }
        }
        stolen
    }

    pub fn len(&self) -> usize {
        self.deque.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.deque.lock().unwrap().is_empty()
    }

    /// Drain all remaining tasks (used during shutdown).
    pub fn drain_all(&self) -> Vec<Arc<RawTask>> {
        self.deque.lock().unwrap().drain(..).collect()
    }
}

/// Global injection queue. External spawns and overflow land here.
/// All workers check this when their local queue is empty.
pub(crate) struct InjectorQueue {
    queue: Mutex<VecDeque<Arc<RawTask>>>,
}

#[allow(dead_code)]
impl InjectorQueue {
    pub fn new() -> Self {
        Self {
            queue: Mutex::new(VecDeque::new()),
        }
    }

    pub fn push(&self, task: Arc<RawTask>) {
        self.queue.lock().unwrap().push_back(task);
    }

    pub fn push_batch(&self, tasks: Vec<Arc<RawTask>>) {
        let mut q = self.queue.lock().unwrap();
        for task in tasks {
            q.push_back(task);
        }
    }

    /// Pop one task from the front.
    pub fn pop(&self) -> Option<Arc<RawTask>> {
        self.queue.lock().unwrap().pop_front()
    }

    /// Pop up to `n` tasks at once (batch drain for workers).
    pub fn pop_batch(&self, n: usize) -> Vec<Arc<RawTask>> {
        let mut q = self.queue.lock().unwrap();
        let count = n.min(q.len());
        q.drain(..count).collect()
    }

    pub fn is_empty(&self) -> bool {
        self.queue.lock().unwrap().is_empty()
    }

    pub fn len(&self) -> usize {
        self.queue.lock().unwrap().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cancel::CancelToken;
    use crate::green::task::RawTask;
    use std::sync::Arc;

    fn dummy_task() -> Arc<RawTask> {
        RawTask::new(
            Box::pin(std::future::ready(())),
            Arc::new(CancelToken::new()),
            "test",
            0,
        )
    }

    #[test]
    fn local_queue_push_pop() {
        let q = LocalQueue::new();
        let t = dummy_task();
        q.push(t).unwrap();
        assert!(!q.is_empty());
        assert!(q.pop().is_some());
        assert!(q.is_empty());
    }

    #[test]
    fn local_queue_overflow() {
        let q = LocalQueue::new();
        for _ in 0..CAPACITY {
            q.push(dummy_task()).unwrap();
        }
        assert!(q.push(dummy_task()).is_err());
    }

    #[test]
    fn local_queue_steal_batch() {
        let q = LocalQueue::new();
        for _ in 0..10 {
            q.push(dummy_task()).unwrap();
        }
        let stolen = q.steal_batch();
        assert_eq!(stolen.len(), 5); // Half
        assert_eq!(q.len(), 5);
    }

    #[test]
    fn local_queue_steal_at_least_one() {
        let q = LocalQueue::new();
        q.push(dummy_task()).unwrap();
        let stolen = q.steal_batch();
        assert_eq!(stolen.len(), 1);
        assert!(q.is_empty());
    }

    #[test]
    fn injector_push_pop() {
        let q = InjectorQueue::new();
        q.push(dummy_task());
        q.push(dummy_task());
        assert_eq!(q.len(), 2);
        assert!(q.pop().is_some());
        assert_eq!(q.len(), 1);
    }

    #[test]
    fn injector_batch_ops() {
        let q = InjectorQueue::new();
        let batch: Vec<_> = (0..5).map(|_| dummy_task()).collect();
        q.push_batch(batch);
        assert_eq!(q.len(), 5);
        let popped = q.pop_batch(3);
        assert_eq!(popped.len(), 3);
        assert_eq!(q.len(), 2);
    }
}

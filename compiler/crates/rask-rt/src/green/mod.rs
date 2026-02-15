// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Phase B green tasks runtime (conc.runtime, conc.strategy/B1-B4).
//!
//! Stackless coroutine tasks on a work-stealing M:N scheduler with
//! epoll-based I/O multiplexing. Replaces Phase A OS-thread internals
//! without changing the programmer-facing API.
//!
//! Components:
//! - `task`      — Task struct, state machine, waker
//! - `queue`     — Work-stealing local queues + global injector
//! - `reactor`   — epoll event loop for I/O readiness
//! - `scheduler` — Worker threads + main polling loop
//! - `io`        — Async read/write/accept futures
//! - `handle`    — GreenTaskHandle (spawn/join/detach/cancel)

pub mod handle;
pub mod io;
pub mod queue;
pub mod reactor;
pub mod scheduler;
pub mod task;

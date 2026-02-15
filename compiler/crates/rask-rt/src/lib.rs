// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Rask runtime library (conc.strategy).
//!
//! Phase A: OS threads. Phase B: M:N green tasks with work-stealing
//! scheduler and epoll reactor. Same `conc.async` API surface — programs
//! don't change between phases.
//!
//! Components:
//! - spawn/join/detach/cancel — thread lifecycle (Phase A)
//! - channels — bounded/unbounded message passing
//! - select — channel multiplexing
//! - sleep/timeout — timer primitives
//! - mutex/shared — sync primitives
//! - green — stackless coroutine scheduler + reactor (Phase B)

pub mod cancel;
pub mod channel;
pub mod context;
pub mod green;
pub mod mutex;
pub mod select;
pub mod shared;
pub mod spawn;
pub mod timeout;

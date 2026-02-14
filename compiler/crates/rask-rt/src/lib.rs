// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Rask Phase A runtime library (conc.strategy).
//!
//! OS threads first. Same `conc.async` API surface, same programmer syntax.
//! Phase B swaps internals to M:N green tasks without source changes.
//!
//! Components:
//! - spawn/join/detach/cancel — thread lifecycle
//! - channels — bounded/unbounded message passing
//! - select — channel multiplexing
//! - sleep/timeout — timer primitives
//! - mutex/shared — sync primitives

pub mod cancel;
pub mod channel;
pub mod context;
pub mod mutex;
pub mod select;
pub mod shared;
pub mod spawn;
pub mod timeout;

// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Methods on threading types: ThreadHandle, Sender, Receiver.
//!
//! Layer: RUNTIME — thread join/detach and channel ops need OS primitives.

use std::sync::{Arc, Mutex, mpsc};

use crate::interp::{Interpreter, RuntimeError};
use crate::value::{ThreadHandleInner, Value};

/// ctrl.panic/O4: a detached task's panic prints to stderr instead of
/// disappearing. `detach()` can't block on the result, so a reaper thread
/// waits for it in the background — the process keeps running either way.
fn report_detached_panic(task_id: i64, jh: std::thread::JoinHandle<Result<Value, String>>) {
    crate::register_detached_reaper(crate::spawn_interp_thread(move || {
        if let Ok(Err(msg)) = jh.join() {
            // F1: say which task, since a runtime task is what died and nobody
            // is going to join it and read the message. Same line as native's.
            eprintln!("task {} panic at {}", task_id, msg);
        }
    }));
}

/// Same as `report_detached_panic`, for tasks submitted to a thread pool
/// (result arrives over a channel instead of a JoinHandle).
fn report_detached_panic_recv(task_id: i64, rx: mpsc::Receiver<Result<Value, String>>) {
    crate::register_detached_reaper(crate::spawn_interp_thread(move || {
        if let Ok(Err(msg)) = rx.recv() {
            eprintln!("task {} panic at {}", task_id, msg);
        }
    }));
}

impl Interpreter {
    /// Mark a handle as consumed in the resource tracker (conc.async/H1).
    fn consume_handle(&mut self, handle: &Arc<ThreadHandleInner>) {
        let ptr = Arc::as_ptr(handle) as usize;
        if let Some(id) = self.resource_tracker.lookup_handle_id(ptr) {
            let _ = self.resource_tracker.mark_consumed(id);
        }
    }

    /// Handle ThreadHandle method calls.
    pub(crate) fn call_thread_handle_method(
        &mut self,
        handle: &Arc<ThreadHandleInner>,
        method: &str,
    ) -> Result<Value, RuntimeError> {
        match method {
            "join" => {
                self.consume_handle(handle);
                let jh = handle.handle.lock().unwrap().take();
                match jh {
                    Some(jh) => match jh.join() {
                        // Thread succeeded - return Ok(value)
                        Ok(Ok(val)) => Ok(Value::Enum {
                            name: "Result".to_string(),
                            variant: "Ok".to_string(),
                            fields: vec![val],
                            variant_index: 0, origin: None,
                        }),
                        // Thread returned error - wrap in JoinError::Panicked
                        Ok(Err(msg)) => Ok(Value::Enum {
                            name: "Result".to_string(),
                            variant: "Err".to_string(),
                            fields: vec![Value::Enum {
                                name: "JoinError".to_string(),
                                variant: "Panicked".to_string(),
                                fields: vec![Value::String(Arc::new(Mutex::new(msg)))],
                                variant_index: 0, origin: None,
                            }],
                            variant_index: 0, origin: None,
                        }),
                        // Thread panicked - return Err(JoinError::Panicked)
                        Err(_) => Ok(Value::Enum {
                            name: "Result".to_string(),
                            variant: "Err".to_string(),
                            fields: vec![Value::Enum {
                                name: "JoinError".to_string(),
                                variant: "Panicked".to_string(),
                                fields: vec![Value::String(Arc::new(Mutex::new(
                                    "thread panicked".to_string(),
                                )))],
                                variant_index: 0, origin: None,
                            }],
                            variant_index: 0, origin: None,
                        }),
                    },
                    // Handle already consumed - return Err(JoinError::Panicked) with message
                    None => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Err".to_string(),
                        fields: vec![Value::Enum {
                            name: "JoinError".to_string(),
                            variant: "Panicked".to_string(),
                            fields: vec![Value::String(Arc::new(Mutex::new(
                                "handle already joined".to_string(),
                            )))],
                            variant_index: 0, origin: None,
                        }],
                        variant_index: 0, origin: None,
                    }),
                }
            }
            "detach" => {
                self.consume_handle(handle);
                if let Some(jh) = handle.handle.lock().unwrap().take() {
                    report_detached_panic(handle.task_id, jh);
                }
                Ok(Value::Unit)
            }
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "ThreadHandle".to_string(),
                method: method.to_string(),
            }),
        }
    }

    /// Handle TaskHandle method calls.
    /// Tasks submitted to a thread pool use the receiver channel; otherwise fall back to join handle.
    pub(crate) fn call_task_handle_method(
        &mut self,
        handle: &Arc<ThreadHandleInner>,
        method: &str,
    ) -> Result<Value, RuntimeError> {
        match method {
            "join" => {
                self.consume_handle(handle);
                // Try receiver first (pool-submitted tasks)
                let rx = handle.receiver.lock().unwrap().take();
                if let Some(rx) = rx {
                    return match rx.recv() {
                        Ok(Ok(val)) => Ok(Value::Enum {
                            name: "Result".to_string(),
                            variant: "Ok".to_string(),
                            fields: vec![val],
                            variant_index: 0, origin: None,
                        }),
                        Ok(Err(msg)) => Ok(Value::Enum {
                            name: "Result".to_string(),
                            variant: "Err".to_string(),
                            fields: vec![Value::Enum {
                                name: "JoinError".to_string(),
                                variant: "Panicked".to_string(),
                                fields: vec![Value::String(Arc::new(Mutex::new(msg)))],
                                variant_index: 0, origin: None,
                            }],
                            variant_index: 0, origin: None,
                        }),
                        Err(_) => Ok(Value::Enum {
                            name: "Result".to_string(),
                            variant: "Err".to_string(),
                            fields: vec![Value::Enum {
                                name: "JoinError".to_string(),
                                variant: "Panicked".to_string(),
                                fields: vec![Value::String(Arc::new(Mutex::new(
                                    "task channel closed".to_string(),
                                )))],
                                variant_index: 0, origin: None,
                            }],
                            variant_index: 0, origin: None,
                        }),
                    };
                }
                // Fall back to OS thread handle
                let jh = handle.handle.lock().unwrap().take();
                match jh {
                    Some(jh) => match jh.join() {
                        Ok(Ok(val)) => Ok(Value::Enum {
                            name: "Result".to_string(),
                            variant: "Ok".to_string(),
                            fields: vec![val],
                            variant_index: 0, origin: None,
                        }),
                        Ok(Err(msg)) => Ok(Value::Enum {
                            name: "Result".to_string(),
                            variant: "Err".to_string(),
                            fields: vec![Value::Enum {
                                name: "JoinError".to_string(),
                                variant: "Panicked".to_string(),
                                fields: vec![Value::String(Arc::new(Mutex::new(msg)))],
                                variant_index: 0, origin: None,
                            }],
                            variant_index: 0, origin: None,
                        }),
                        Err(_) => Ok(Value::Enum {
                            name: "Result".to_string(),
                            variant: "Err".to_string(),
                            fields: vec![Value::Enum {
                                name: "JoinError".to_string(),
                                variant: "Panicked".to_string(),
                                fields: vec![Value::String(Arc::new(Mutex::new(
                                    "task panicked".to_string(),
                                )))],
                                variant_index: 0, origin: None,
                            }],
                            variant_index: 0, origin: None,
                        }),
                    },
                    None => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Err".to_string(),
                        fields: vec![Value::Enum {
                            name: "JoinError".to_string(),
                            variant: "Panicked".to_string(),
                            fields: vec![Value::String(Arc::new(Mutex::new(
                                "handle already joined".to_string(),
                            )))],
                            variant_index: 0, origin: None,
                        }],
                        variant_index: 0, origin: None,
                    }),
                }
            }
            "detach" => {
                self.consume_handle(handle);
                // O4: detach doesn't wait, but the eventual panic (if any)
                // still has to reach stderr — hand it to a reaper thread
                // instead of dropping the result.
                if let Some(rx) = handle.receiver.lock().unwrap().take() {
                    report_detached_panic_recv(handle.task_id, rx);
                } else if let Some(jh) = handle.handle.lock().unwrap().take() {
                    report_detached_panic(handle.task_id, jh);
                }
                Ok(Value::Unit)
            }
            "cancel" => {
                self.consume_handle(handle);
                // Cooperative cancellation (CN1): set flag and join.
                // Phase A: no cancel token in interpreter yet — just join and
                // return Cancelled. Full cancel support lives in the C runtime.
                let jh = handle.handle.lock().unwrap().take();
                match jh {
                    Some(jh) => {
                        let _ = jh.join();
                        Ok(Value::Enum {
                            name: "Result".to_string(),
                            variant: "Err".to_string(),
                            fields: vec![Value::Enum {
                                name: "JoinError".to_string(),
                                variant: "Cancelled".to_string(),
                                fields: vec![],
                                variant_index: 0, origin: None,
                            }],
                            variant_index: 0, origin: None,
                        })
                    }
                    None => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Err".to_string(),
                        fields: vec![Value::Enum {
                            name: "JoinError".to_string(),
                            variant: "Panicked".to_string(),
                            fields: vec![Value::String(Arc::new(Mutex::new(
                                "handle already consumed".to_string(),
                            )))],
                            variant_index: 0, origin: None,
                        }],
                        variant_index: 0, origin: None,
                    }),
                }
            }
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "TaskHandle".to_string(),
                method: method.to_string(),
            }),
        }
    }

    /// Handle Sender method calls.
    pub(crate) fn call_sender_method(
        &self,
        tx: &Arc<Mutex<mpsc::SyncSender<Value>>>,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match method {
            "send" => {
                let val = args.into_iter().next().unwrap_or(Value::Unit);
                let tx = tx.lock().unwrap();
                match tx.send(val) {
                    Ok(()) => Ok(chan_ok(Value::Unit)),
                    Err(_) => Ok(chan_err(chan_error("SendError", "Closed", 0, vec![]))),
                }
            }
            "try_send" => {
                let val = args.into_iter().next().unwrap_or(Value::Unit);
                let tx = tx.lock().unwrap();
                match tx.try_send(val) {
                    Ok(()) => Ok(chan_ok(Value::Unit)),
                    // Both variants carry the value back — the send didn't
                    // happen, so the caller still owns what it tried to send.
                    Err(mpsc::TrySendError::Full(v)) => {
                        Ok(chan_err(chan_error("TrySendError", "Full", 0, vec![v])))
                    }
                    Err(mpsc::TrySendError::Disconnected(v)) => {
                        Ok(chan_err(chan_error("TrySendError", "Closed", 1, vec![v])))
                    }
                }
            }
            "close" => {
                // Drop the sender to close the channel
                let mut guard = tx.lock().unwrap();
                // Replace with a disconnected sender by dropping the inner value
                // We can't actually drop through Arc<Mutex<>>, so we create a
                // disconnected channel and swap in its sender.
                let (replacement, _) = mpsc::sync_channel(0);
                *guard = replacement;
                Ok(Value::Enum {
                    name: "Result".to_string(),
                    variant: "Ok".to_string(),
                    fields: vec![Value::Unit],
                    variant_index: 0, origin: None,
                })
            }
            "clone" => {
                // A cloned sender is another handle to the same channel.
                let inner = tx.lock().unwrap().clone();
                Ok(Value::Sender(Arc::new(Mutex::new(inner))))
            }
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "Sender".to_string(),
                method: method.to_string(),
            }),
        }
    }

    /// Handle Receiver method calls.
    pub(crate) fn call_receiver_method(
        &self,
        rx: &Arc<Mutex<mpsc::Receiver<Value>>>,
        method: &str,
    ) -> Result<Value, RuntimeError> {
        match method {
            "receive" => {
                let rx = rx.lock().unwrap();
                match rx.recv() {
                    Ok(val) => Ok(chan_ok(val)),
                    Err(_) => Ok(chan_err(chan_error("ReceiveError", "Closed", 0, vec![]))),
                }
            }
            "try_receive" => {
                let rx = rx.lock().unwrap();
                match rx.try_recv() {
                    Ok(val) => Ok(chan_ok(val)),
                    Err(mpsc::TryRecvError::Empty) => {
                        Ok(chan_err(chan_error("TryReceiveError", "Empty", 0, vec![])))
                    }
                    Err(mpsc::TryRecvError::Disconnected) => {
                        Ok(chan_err(chan_error("TryReceiveError", "Closed", 1, vec![])))
                    }
                }
            }
            "close" => {
                // Drop the receiver to close the channel
                let mut guard = rx.lock().unwrap();
                // Replace with a disconnected receiver
                let (_, replacement) = mpsc::sync_channel(0);
                *guard = replacement;
                Ok(Value::Enum {
                    name: "Result".to_string(),
                    variant: "Ok".to_string(),
                    fields: vec![Value::Unit],
                    variant_index: 0, origin: None,
                })
            }
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "Receiver".to_string(),
                method: method.to_string(),
            }),
        }
    }
}

/// `Ok(value)` for a channel operation.
fn chan_ok(value: Value) -> Value {
    Value::Enum {
        name: "Result".to_string(),
        variant: "Ok".to_string(),
        fields: vec![value],
        variant_index: 0,
        origin: None,
    }
}

/// `Err(error)` for a channel operation. The tag is 1 — `Err` was built with
/// index 0 here, the same number `Ok` uses.
fn chan_err(error: Value) -> Value {
    Value::Enum {
        name: "Result".to_string(),
        variant: "Err".to_string(),
        fields: vec![error],
        variant_index: 1,
        origin: None,
    }
}

/// One of `stdlib/async.rk`'s channel error enums.
///
/// These used to be bare strings, so `match rx.receive() { ReceiveError as e =>
/// … }` matched no arm and `e.message()` had nothing to resolve against — the
/// error branch the signature promises was unreachable (#1067).
fn chan_error(ty: &str, variant: &str, index: u32, fields: Vec<Value>) -> Value {
    Value::Enum {
        name: ty.to_string(),
        variant: variant.to_string(),
        fields,
        variant_index: index,
        origin: None,
    }
}

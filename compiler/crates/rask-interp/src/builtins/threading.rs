// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Methods on threading types: ThreadHandle, Sender, Receiver.
//!
//! Layer: RUNTIME — thread join/detach and channel ops need OS primitives.

use std::sync::{Arc, Mutex, mpsc};

use crate::interp::{Interpreter, RuntimeError};
use crate::value::{ThreadHandleInner, Value};

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
                        }),
                        // Thread returned error - wrap in JoinError::Panicked
                        Ok(Err(msg)) => Ok(Value::Enum {
                            name: "Result".to_string(),
                            variant: "Err".to_string(),
                            fields: vec![Value::Enum {
                                name: "JoinError".to_string(),
                                variant: "Panicked".to_string(),
                                fields: vec![Value::String(Arc::new(Mutex::new(msg)))],
                            }],
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
                            }],
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
                        }],
                    }),
                }
            }
            "detach" => {
                self.consume_handle(handle);
                let _ = handle.handle.lock().unwrap().take();
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
                        }),
                        Ok(Err(msg)) => Ok(Value::Enum {
                            name: "Result".to_string(),
                            variant: "Err".to_string(),
                            fields: vec![Value::Enum {
                                name: "JoinError".to_string(),
                                variant: "Panicked".to_string(),
                                fields: vec![Value::String(Arc::new(Mutex::new(msg)))],
                            }],
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
                            }],
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
                        }),
                        Ok(Err(msg)) => Ok(Value::Enum {
                            name: "Result".to_string(),
                            variant: "Err".to_string(),
                            fields: vec![Value::Enum {
                                name: "JoinError".to_string(),
                                variant: "Panicked".to_string(),
                                fields: vec![Value::String(Arc::new(Mutex::new(msg)))],
                            }],
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
                            }],
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
                        }],
                    }),
                }
            }
            "detach" => {
                self.consume_handle(handle);
                let _ = handle.handle.lock().unwrap().take();
                let _ = handle.receiver.lock().unwrap().take();
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
                            }],
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
                        }],
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
                    Ok(()) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Ok".to_string(),
                        fields: vec![Value::Unit],
                    }),
                    Err(_) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Err".to_string(),
                        fields: vec![Value::String(Arc::new(Mutex::new(
                            "channel closed".to_string(),
                        )))],
                    }),
                }
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
            "recv" => {
                let rx = rx.lock().unwrap();
                match rx.recv() {
                    Ok(val) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Ok".to_string(),
                        fields: vec![val],
                    }),
                    Err(_) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Err".to_string(),
                        fields: vec![Value::String(Arc::new(Mutex::new(
                            "channel closed".to_string(),
                        )))],
                    }),
                }
            }
            "try_recv" => {
                let rx = rx.lock().unwrap();
                match rx.try_recv() {
                    Ok(val) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Ok".to_string(),
                        fields: vec![val],
                    }),
                    Err(mpsc::TryRecvError::Empty) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Err".to_string(),
                        fields: vec![Value::String(Arc::new(Mutex::new("empty".to_string())))],
                    }),
                    Err(mpsc::TryRecvError::Disconnected) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Err".to_string(),
                        fields: vec![Value::String(Arc::new(Mutex::new(
                            "channel closed".to_string(),
                        )))],
                    }),
                }
            }
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "Receiver".to_string(),
                method: method.to_string(),
            }),
        }
    }
}

// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Methods on threading types: Handle, Sender, Receiver.
//!
//! Layer: RUNTIME — thread join/detach and channel ops need OS primitives.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, mpsc};

use crate::interp::{Interpreter, RuntimeError};
use crate::value::{HandleInner, Value};

/// ctrl.panic/O4: a detached task's panic prints to stderr instead of
/// disappearing. `detach()` can't block on the result, so a reaper thread
/// waits for it in the background — the process keeps running either way.
fn report_detached_panic(task_id: i64, jh: std::thread::JoinHandle<Result<Value, String>>) {
    // Reaching here means a task is already running, so the target has threads
    // and the spawn can't fail for want of them. If it fails anyway there is
    // nothing to reap and nobody to tell, so the reaper is simply not
    // registered (#1172).
    if let Ok(reaper) = crate::spawn_interp_thread(move || {
        if let Ok(Err(msg)) = jh.join() {
            // F1: say which task, since a runtime task is what died and nobody
            // is going to join it and read the message. Same line as native's.
            eprintln!("task {} panic at {}", task_id, msg);
        }
    }) {
        crate::register_detached_reaper(reaper);
    }
}

impl Interpreter {
    /// Mark a handle as consumed in the resource tracker (conc.async/H1).
    fn consume_handle(&mut self, handle: &Arc<HandleInner>) {
        let ptr = Arc::as_ptr(handle) as usize;
        if let Some(id) = self.resource_tracker.lookup_handle_id(ptr) {
            let _ = self.resource_tracker.mark_consumed(id);
        }
    }

    /// `Handle` methods — the same for a task, a pooled job and a thread.
    pub(crate) fn call_handle_method(
        &mut self,
        handle: &Arc<HandleInner>,
        method: &str,
    ) -> Result<Value, RuntimeError> {
        match method {
            "join" => {
                self.consume_handle(handle);
                Ok(join_outcome(handle))
            }
            "cancel" => {
                self.consume_handle(handle);
                // CN1: raise the flag, then wait for the body to notice.
                handle.cancel.store(true, Ordering::Release);
                Ok(join_outcome(handle))
            }
            "detach" => {
                self.consume_handle(handle);
                if let Some(jh) = handle.handle.lock().unwrap().take() {
                    report_detached_panic(handle.task_id, jh);
                }
                Ok(Value::Unit)
            }
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "Handle".to_string(),
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

/// Wait for the body and say how it ended, as `T or JoinError`. Cancelled wins
/// over a value, the same as native: the caller asked it to stop, so what it
/// returned on the way out isn't the answer.
fn join_outcome(handle: &HandleInner) -> Value {
    let jh = handle.handle.lock().unwrap().take();
    // Without the slot: a joiner that kept it would leave
    // `using Multitasking(workers: 1)` with nothing free to run the task it is
    // waiting for (#1111).
    let ended = match jh {
        Some(jh) => crate::without_task_slot(|| jh.join())
            .unwrap_or_else(|_| Err("task panicked".to_string())),
        None => Err("handle already consumed".to_string()),
    };
    let failed = |variant: &str, fields: Vec<Value>| Value::Enum {
        name: "Result".to_string(),
        variant: "Err".to_string(),
        fields: vec![Value::Enum {
            name: "JoinError".to_string(),
            variant: variant.to_string(),
            fields,
            variant_index: 0, origin: None,
        }],
        variant_index: 0, origin: None,
    };
    match ended {
        Err(msg) => failed("Panicked", vec![Value::String(Arc::new(Mutex::new(msg)))]),
        Ok(_) if handle.cancel.load(Ordering::Acquire) => failed("Cancelled", vec![]),
        Ok(val) => Value::Enum {
            name: "Result".to_string(),
            variant: "Ok".to_string(),
            fields: vec![val],
            variant_index: 0, origin: None,
        },
    }
}

//! Methods on threading types: ThreadHandle, Sender, Receiver.

use std::sync::{Arc, Mutex, mpsc};

use crate::interp::{Interpreter, RuntimeError};
use crate::value::{ThreadHandleInner, Value};

impl Interpreter {
    /// Handle ThreadHandle method calls.
    pub(crate) fn call_thread_handle_method(
        &self,
        handle: &Arc<ThreadHandleInner>,
        method: &str,
    ) -> Result<Value, RuntimeError> {
        match method {
            "join" => {
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
                            fields: vec![Value::String(Arc::new(Mutex::new(msg)))],
                        }),
                        Err(_) => Ok(Value::Enum {
                            name: "Result".to_string(),
                            variant: "Err".to_string(),
                            fields: vec![Value::String(Arc::new(Mutex::new(
                                "thread panicked".to_string(),
                            )))],
                        }),
                    },
                    None => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Err".to_string(),
                        fields: vec![Value::String(Arc::new(Mutex::new(
                            "handle already joined".to_string(),
                        )))],
                    }),
                }
            }
            "detach" => {
                let _ = handle.handle.lock().unwrap().take();
                Ok(Value::Unit)
            }
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "ThreadHandle".to_string(),
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

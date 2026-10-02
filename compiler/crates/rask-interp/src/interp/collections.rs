// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Collection indexing and writeback.

use crate::value::Value;

use super::{Interpreter, RuntimeError};

impl Interpreter {
    pub(super) fn index_into(&mut self, collection: &Value, key: &Value) -> Result<Value, RuntimeError> {
        match (collection, key) {
            (Value::Vec(v), Value::Int(i, _)) => {
                let vec = v.lock().unwrap();
                vec.get(*i as usize).cloned().ok_or_else(|| {
                    RuntimeError::IndexOutOfBounds { index: *i, len: vec.len() }
                })
            }
            (Value::Map(m), _) => self
                .map_get(m, key.clone())?
                .ok_or_else(|| RuntimeError::Panic("key not found in map".to_string())),
            _ => Err(RuntimeError::TypeError(format!(
                "with...as: cannot index into {}", collection.type_name()
            ))),
        }
    }

    /// Write a value back to a collection at the given key (for with...as writeback).
    pub(super) fn write_back_index(&mut self, collection: &Value, key: &Value, value: Value) -> Result<(), RuntimeError> {
        match (collection, key) {
            (Value::Vec(v), Value::Int(i, _)) => {
                let mut vec = v.lock().unwrap();
                let idx = *i as usize;
                if idx < vec.len() {
                    vec[idx] = value;
                    Ok(())
                } else {
                    Err(RuntimeError::IndexOutOfBounds { index: *i, len: vec.len() })
                }
            }
            (Value::Map(m), _) => {
                self.map_insert(m, key.clone(), value)?;
                Ok(())
            }
            _ => Err(RuntimeError::TypeError(format!(
                "with...as: cannot write back to {}", collection.type_name()
            ))),
        }
    }
}


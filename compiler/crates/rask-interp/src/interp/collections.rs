// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Collection indexing and writeback.

use crate::value::Value;

use super::{Interpreter, RuntimeError};

impl Interpreter {
    pub(super) fn index_into(&self, collection: &Value, key: &Value) -> Result<Value, RuntimeError> {
        match (collection, key) {
            (Value::Vec(v), Value::Int(i)) => {
                let vec = v.lock().unwrap();
                vec.get(*i as usize).cloned().ok_or_else(|| {
                    RuntimeError::IndexOutOfBounds { index: *i, len: vec.len() }
                })
            }
            (Value::Pool(p), Value::Handle { pool_id, index, generation }) => {
                let pool = p.lock().unwrap();
                let slot_idx = pool.validate(*pool_id, *index, *generation)
                    .map_err(RuntimeError::Panic)?;
                pool.slots[slot_idx].1.clone().ok_or_else(|| {
                    RuntimeError::Panic("pool slot is empty".to_string())
                })
            }
            (Value::Map(m), _) => {
                let map = m.lock().unwrap();
                for (k, v) in map.iter() {
                    if Self::value_eq(k, key) {
                        return Ok(v.clone());
                    }
                }
                Err(RuntimeError::Panic("key not found in map".to_string()))
            }
            _ => Err(RuntimeError::TypeError(format!(
                "with...as: cannot index into {}", collection.type_name()
            ))),
        }
    }

    /// Write a value back to a collection at the given key (for with...as writeback).
    pub(super) fn write_back_index(&self, collection: &Value, key: &Value, value: Value) -> Result<(), RuntimeError> {
        match (collection, key) {
            (Value::Vec(v), Value::Int(i)) => {
                let mut vec = v.lock().unwrap();
                let idx = *i as usize;
                if idx < vec.len() {
                    vec[idx] = value;
                    Ok(())
                } else {
                    Err(RuntimeError::IndexOutOfBounds { index: *i, len: vec.len() })
                }
            }
            (Value::Pool(p), Value::Handle { pool_id, index, generation }) => {
                let mut pool = p.lock().unwrap();
                let slot_idx = pool.validate(*pool_id, *index, *generation)
                    .map_err(RuntimeError::Panic)?;
                pool.slots[slot_idx].1 = Some(value);
                Ok(())
            }
            (Value::Map(m), _) => {
                let mut map = m.lock().unwrap();
                for (k, v) in map.iter_mut() {
                    if Self::value_eq(k, key) {
                        *v = value;
                        return Ok(());
                    }
                }
                map.push((key.clone(), value));
                Ok(())
            }
            _ => Err(RuntimeError::TypeError(format!(
                "with...as: cannot write back to {}", collection.type_name()
            ))),
        }
    }
}


// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Map keys through their type's own `eq` and `hash`.
//!
//! A map key is the same key as another when `==` says so, and it buckets by
//! what `.hash()` says. Both are methods: derived for a struct or enum (written
//! by the desugarer), the owner's own when it implements `Equal`/`Hashable`,
//! and element by element for a `Vec` (in `collections.rk`). The interpreter
//! used to compare keys structurally, which agreed with the derived methods and
//! ignored the owner's: a `Version` equal by major number alone was two keys
//! (#1391).

use std::sync::{Arc, Mutex};

use indexmap::map::RawEntryApiV1;

use crate::value::{MapData, MapKey, Value};

use super::{Interpreter, RuntimeError};

impl Interpreter {
    /// `key` as a map stores it.
    pub(crate) fn map_key(&mut self, key: Value) -> Result<MapKey, RuntimeError> {
        // A key with no `hash` is a key problem, not the map's: let a
        // `NoSuchMethod` through and it reads as "Map has no `insert`".
        let answer = match self.call_method(key.clone(), "hash", vec![], None) {
            Err(RuntimeError::NoSuchMethod { ty, .. }) => {
                return Err(RuntimeError::TypeError(format!("a `{ty}` can't key a map: it has no `hash`")))
            }
            other => other?,
        };
        let hash = match answer {
            Value::Int(n, _) => n as u64,
            other => {
                return Err(RuntimeError::TypeError(format!(
                    "a map key's `hash` answered {}, not a u64",
                    other.type_name()
                )))
            }
        };
        Ok(MapKey { value: key, hash })
    }

    fn keys_equal(&mut self, a: &Value, b: &Value) -> Result<bool, RuntimeError> {
        match self.call_method(a.clone(), "eq", vec![b.clone()], None)? {
            Value::Bool(eq) => Ok(eq),
            other => Err(RuntimeError::TypeError(format!(
                "a map key's `eq` answered {}, not a bool",
                other.type_name()
            ))),
        }
    }

    /// Where `key` sits in `map`, comparing with the key type's `eq`.
    pub(crate) fn map_index(&mut self, map: &MapData, key: &MapKey) -> Result<Option<usize>, RuntimeError> {
        use std::hash::BuildHasher;
        let mut failed = None;
        // The table is keyed by the map's own hasher run over `MapKey`, which
        // feeds it `key.hash`; asking with the raw number misses every bucket.
        let bucket = map.hasher().hash_one(key);
        let found = map.raw_entry_v1().index_from_hash(bucket, |stored| {
            if failed.is_some() {
                return false;
            }
            match self.keys_equal(&stored.value, &key.value) {
                Ok(eq) => eq,
                Err(e) => {
                    failed = Some(e);
                    false
                }
            }
        });
        match failed {
            Some(e) => Err(e),
            None => Ok(found),
        }
    }

    pub(crate) fn map_get(&mut self, m: &Arc<Mutex<MapData>>, key: Value) -> Result<Option<Value>, RuntimeError> {
        let key = self.map_key(key)?;
        let map = m.lock().unwrap();
        Ok(self.map_index(&map, &key)?.map(|i| map[i].clone()))
    }

    pub(crate) fn map_contains(&mut self, m: &Arc<Mutex<MapData>>, key: Value) -> Result<bool, RuntimeError> {
        let key = self.map_key(key)?;
        let map = m.lock().unwrap();
        Ok(self.map_index(&map, &key)?.is_some())
    }

    /// Insert, answering the value it displaced. An equal key already there
    /// stays: the map holds the first spelling it was given.
    pub(crate) fn map_insert(
        &mut self,
        m: &Arc<Mutex<MapData>>,
        key: Value,
        value: Value,
    ) -> Result<Option<Value>, RuntimeError> {
        let key = self.map_key(key)?;
        let mut map = m.lock().unwrap();
        match self.map_index(&map, &key)? {
            Some(i) => Ok(Some(std::mem::replace(&mut map[i], value))),
            None => {
                map.insert(key, value);
                Ok(None)
            }
        }
    }

    pub(crate) fn map_remove(&mut self, m: &Arc<Mutex<MapData>>, key: Value) -> Result<Option<Value>, RuntimeError> {
        let key = self.map_key(key)?;
        let mut map = m.lock().unwrap();
        Ok(match self.map_index(&map, &key)? {
            Some(i) => map.swap_remove_index(i).map(|(_, v)| v),
            None => None,
        })
    }
}

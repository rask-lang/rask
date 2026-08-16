// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! `Wide<T>` — staged data-parallel plans on the CPU (conc.data-parallel).
//!
//! `map`/`zip_with` stage lazily (return a new plan, run nothing). `read`/`sum`
//! are terminals: they execute the plan on the CPU and return a result. This is
//! the interpreter's realization of the "stage → run" model; the CPU result is
//! the reference semantics a device backend must match (conc.data-parallel/W3).

use std::sync::{Arc, Mutex};

use crate::interp::{Interpreter, RuntimeError};
use crate::value::{FloatKind, Value, WidePlan};

impl Interpreter {
    /// Execute a plan into its lane vector. This is the one place work happens.
    fn eval_wide(&mut self, plan: &WidePlan) -> Result<Vec<Value>, RuntimeError> {
        match plan {
            WidePlan::Source(items) => Ok(items.lock().unwrap().items.clone()),
            WidePlan::Map { source, mapper } => {
                let lanes = self.eval_wide(source)?;
                let mut out = Vec::with_capacity(lanes.len());
                for item in lanes {
                    out.push(self.call_value(mapper.clone(), vec![item])?);
                }
                Ok(out)
            }
            WidePlan::ZipWith { a, b, combiner } => {
                let la = self.eval_wide(a)?;
                let lb = self.eval_wide(b)?;
                if la.len() != lb.len() {
                    return Err(RuntimeError::TypeError(format!(
                        "zip_with: lane counts differ ({} vs {})",
                        la.len(),
                        lb.len()
                    )));
                }
                let mut out = Vec::with_capacity(la.len());
                for (x, y) in la.into_iter().zip(lb.into_iter()) {
                    out.push(self.call_value(combiner.clone(), vec![x, y])?);
                }
                Ok(out)
            }
        }
    }

    /// Dispatch a method on a `Wide<T>` value.
    pub(crate) fn call_wide_method(
        &mut self,
        plan: &Arc<WidePlan>,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match method {
            // --- staging (lazy) ---
            "map" => {
                let mapper = args.into_iter().next().unwrap_or(Value::Unit);
                Ok(Value::Wide(Arc::new(WidePlan::Map {
                    source: Arc::clone(plan),
                    mapper,
                })))
            }
            "zip_with" => {
                let mut it = args.into_iter();
                let other = it.next().unwrap_or(Value::Unit);
                let combiner = it.next().unwrap_or(Value::Unit);
                let b = match other {
                    Value::Wide(p) => p,
                    v => {
                        return Err(RuntimeError::TypeError(format!(
                            "zip_with expects a Wide, got {}",
                            v.type_name()
                        )))
                    }
                };
                Ok(Value::Wide(Arc::new(WidePlan::ZipWith {
                    a: Arc::clone(plan),
                    b,
                    combiner,
                })))
            }
            // --- terminals (run the plan) ---
            "read" => {
                let lanes = self.eval_wide(plan)?;
                Ok(Value::vec(lanes))
            }
            "sum" => {
                let lanes = self.eval_wide(plan)?;
                sum_lanes(&lanes)
            }
            "reduce" => {
                let mut it = args.into_iter();
                let mut acc = it.next().unwrap_or(Value::Unit);
                let f = it.next().unwrap_or(Value::Unit);
                let lanes = self.eval_wide(plan)?;
                for item in lanes {
                    acc = self.call_value(f.clone(), vec![acc, item])?;
                }
                Ok(acc)
            }
            "min" => {
                let lanes = self.eval_wide(plan)?;
                extreme_lane(&lanes, method)
            }
            "max" => {
                let lanes = self.eval_wide(plan)?;
                extreme_lane(&lanes, method)
            }
            other => Err(RuntimeError::TypeError(format!(
                "Wide has no method `{}`",
                other
            ))),
        }
    }
}

/// Reduce lanes with `+`. Matches Vec.sum: int unless a float appears.
fn sum_lanes(lanes: &[Value]) -> Result<Value, RuntimeError> {
    let mut sum = 0i64;
    let mut float_sum = 0.0f64;
    let mut is_float = false;
    for item in lanes {
        match item {
            Value::Int(n, _) => {
                if is_float {
                    float_sum += *n as f64;
                } else {
                    sum += n;
                }
            }
            Value::Float(f, _) => {
                if !is_float {
                    float_sum = sum as f64 + f;
                    is_float = true;
                } else {
                    float_sum += f;
                }
            }
            _ => {
                return Err(RuntimeError::TypeError(format!(
                    "sum requires numeric values, got {}",
                    item.type_name()
                )))
            }
        }
    }
    if is_float {
        Ok(Value::Float(float_sum, FloatKind::Untyped))
    } else {
        Ok(Value::int(sum))
    }
}

/// Numeric value of a lane for min/max comparison.
fn numeric(v: &Value) -> Option<f64> {
    match v {
        Value::Int(n, _) => Some(*n as f64),
        Value::Float(f, _) => Some(*f),
        _ => None,
    }
}

/// `min`/`max` over lanes. Errors on an empty plan or non-numeric lanes.
fn extreme_lane(lanes: &[Value], which: &str) -> Result<Value, RuntimeError> {
    let mut best: Option<Value> = None;
    let mut best_key = 0.0f64;
    for item in lanes {
        let k = numeric(item).ok_or_else(|| {
            RuntimeError::TypeError(format!("{} requires numeric lanes, got {}", which, item.type_name()))
        })?;
        let take = match &best {
            None => true,
            Some(_) if which == "min" => k < best_key,
            Some(_) => k > best_key,
        };
        if take {
            best = Some(item.clone());
            best_key = k;
        }
    }
    best.ok_or_else(|| RuntimeError::TypeError(format!("{} on an empty Wide", which)))
}

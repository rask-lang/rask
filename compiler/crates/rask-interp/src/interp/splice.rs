// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! `comptime for` and `comptime if` among a list's entries (ctrl.comptime/CT69).
//!
//! The same shape as native's (rask-mir `lower/splice.rs`): a list with one
//! is flattened first — each entry copied once per item it was written
//! under, a computed label worked out (CT71) — and then evaluated like a list
//! written out by hand. Each copy carries the items it was written under, and
//! `eval_expr`/`locate` bind them while they evaluate that copy. Copies are
//! told apart by address, since they share the written entry's node ids.

use rask_ast::expr::{CallArg, Expr, ExprKind, FieldInit, Splice};
use rask_ast::stmt::ForBinding;

use super::{Interpreter, RuntimeDiagnostic, RuntimeError};
use crate::value::Value;

/// The loop items in scope where an entry was written, outermost first.
pub(crate) type EntryItems = Vec<(String, Value)>;

impl Interpreter {
    pub(crate) fn is_spliced<'e>(mut exprs: impl Iterator<Item = &'e Expr>) -> bool {
        exprs.any(|e| matches!(e.kind, ExprKind::Splice(_)))
    }

    /// The entries the list has, each with the items it was written under.
    pub(crate) fn flatten_entries(&mut self, entries: &[CallArg]) -> Result<Vec<(CallArg, EntryItems)>, RuntimeDiagnostic> {
        let mut out = Vec::new();
        let mut items = Vec::new();
        self.flatten_into(entries, &mut items, &mut out)?;
        Ok(out)
    }

    fn flatten_into(
        &mut self,
        entries: &[CallArg],
        items: &mut EntryItems,
        out: &mut Vec<(CallArg, EntryItems)>,
    ) -> Result<(), RuntimeDiagnostic> {
        for entry in entries {
            let ExprKind::Splice(splice) = &entry.expr.kind else {
                let mut entry = entry.clone();
                if let Some(label) = entry.computed_label.take() {
                    let name = match self.eval_in_items(items.clone(), |this| this.eval_expr(&label))? {
                        Value::String(s) => s.lock().unwrap().clone(),
                        _ => {
                            return Err(RuntimeDiagnostic::new(
                                RuntimeError::TypeError("a computed label has to be a field's name (ctrl.comptime/CT71)".into()),
                                label.span,
                            ))
                        }
                    };
                    entry.name = Some(name);
                }
                out.push((entry, items.clone()));
                continue;
            };
            match &**splice {
                Splice::For { binding, iter, entries } => {
                    let ForBinding::Single(name) = binding else {
                        return Err(RuntimeDiagnostic::new(
                            RuntimeError::TypeError("comptime for requires a single binding".into()),
                            entry.expr.span,
                        ));
                    };
                    let listed = match self.eval_in_items(items.clone(), |this| this.eval_expr(iter))? {
                        Value::Vec(v) => v.lock().unwrap().items.clone(),
                        _ => {
                            return Err(RuntimeDiagnostic::new(
                                RuntimeError::TypeError("a `comptime for` among a list's entries needs a comptime list to walk".into()),
                                iter.span,
                            ))
                        }
                    };
                    for item in listed {
                        items.push((name.clone(), item));
                        let result = self.flatten_into(entries, items, out);
                        items.pop();
                        result?;
                    }
                }
                Splice::If { cond, then_entries, else_entries } => {
                    let taken = match self.eval_in_items(items.clone(), |this| this.eval_expr(cond))? {
                        Value::Bool(b) => b,
                        _ => {
                            return Err(RuntimeDiagnostic::new(
                                RuntimeError::TypeError("a `comptime if` condition is a bool".into()),
                                cond.span,
                            ))
                        }
                    };
                    self.flatten_into(if taken { then_entries } else { else_entries }, items, out)?;
                }
            }
        }
        Ok(())
    }

    /// A struct literal's fields, as the call it was written as.
    pub(crate) fn flatten_fields(&mut self, fields: &[FieldInit]) -> Result<Vec<(FieldInit, EntryItems)>, RuntimeDiagnostic> {
        let as_args: Vec<CallArg> = fields
            .iter()
            .map(|f| match &f.value.kind {
                ExprKind::Splice(_) => CallArg::plain(f.value.clone()),
                _ => CallArg { name: Some(f.name.clone()), ..CallArg::plain(f.value.clone()) },
            })
            .collect();
        self.flatten_entries(&as_args)?
            .into_iter()
            .map(|(arg, items)| match arg.name {
                Some(name) => Ok((FieldInit { name, value: arg.expr }, items)),
                None => Err(RuntimeDiagnostic::new(
                    RuntimeError::TypeError(
                        "a struct literal's `comptime for` gives fields by name: `(f.name): value` (ctrl.comptime/CT71)".into(),
                    ),
                    arg.expr.span,
                )),
            })
            .collect()
    }

    /// Run `eval` with `items` bound, each in a scope of its own.
    pub(crate) fn eval_in_items<T>(
        &mut self,
        items: EntryItems,
        eval: impl FnOnce(&mut Self) -> Result<T, RuntimeDiagnostic>,
    ) -> Result<T, RuntimeDiagnostic> {
        if items.is_empty() {
            return eval(self);
        }
        self.env.push_scope();
        for (name, value) in items {
            self.env.define(name, value);
        }
        let result = eval(self);
        self.env.pop_scope();
        result
    }

    pub(crate) fn scope_entries<'e>(&mut self, scoped: impl Iterator<Item = (&'e Expr, &'e EntryItems)>) {
        for (expr, items) in scoped {
            self.entry_items.insert(expr as *const Expr as usize, items.clone());
        }
    }

    pub(crate) fn unscope_entries<'e>(&mut self, exprs: impl Iterator<Item = &'e Expr>) {
        for expr in exprs {
            self.entry_items.remove(&(expr as *const Expr as usize));
        }
    }

    /// The items `expr` is evaluated under, when it's a copy a list made.
    /// Taken out while it runs, so the evaluation underneath doesn't find it.
    pub(crate) fn take_entry_scope(&mut self, expr: &Expr) -> Option<EntryItems> {
        self.entry_items.remove(&(expr as *const Expr as usize))
    }

    pub(crate) fn put_entry_scope(&mut self, expr: &Expr, items: EntryItems) {
        self.entry_items.insert(expr as *const Expr as usize, items);
    }

    /// `expr` with its list's `comptime for`/`comptime if` entries flattened,
    /// or `None` when it has none. The copies are registered; call
    /// `unscope_rebuilt` once it's evaluated.
    pub(crate) fn rebuild_spliced(&mut self, expr: &Expr) -> Result<Option<Expr>, RuntimeDiagnostic> {
        match &expr.kind {
            ExprKind::Call { func, args } if Self::is_spliced(args.iter().map(|a| &a.expr)) => {
                let (args, items) = unzip(self.flatten_entries(args)?);
                return Ok(Some(self.registered(expr, ExprKind::Call { func: func.clone(), args }, items)));
            }
            ExprKind::MethodCall { object, method, type_args, args } if Self::is_spliced(args.iter().map(|a| &a.expr)) => {
                let (args, items) = unzip(self.flatten_entries(args)?);
                let kind = ExprKind::MethodCall { object: object.clone(), method: method.clone(), type_args: type_args.clone(), args };
                Ok(Some(self.registered(expr, kind, items)))
            }
            ExprKind::Array(elems) if Self::is_spliced(elems.iter()) => {
                let as_args: Vec<CallArg> = elems.iter().cloned().map(CallArg::plain).collect();
                let (args, items) = unzip(self.flatten_entries(&as_args)?);
                let elems = args.into_iter().map(|a| a.expr).collect();
                return Ok(Some(self.registered(expr, ExprKind::Array(elems), items)));
            }
            ExprKind::Tuple(elems) if Self::is_spliced(elems.iter()) => {
                let as_args: Vec<CallArg> = elems.iter().cloned().map(CallArg::plain).collect();
                let (args, items) = unzip(self.flatten_entries(&as_args)?);
                let elems = args.into_iter().map(|a| a.expr).collect();
                return Ok(Some(self.registered(expr, ExprKind::Tuple(elems), items)));
            }
            ExprKind::StructLit { name, type_args, fields, spread } if Self::is_spliced(fields.iter().map(|f| &f.value)) => {
                let flat = self.flatten_fields(fields)?;
                let (fields, items): (Vec<FieldInit>, Vec<EntryItems>) = flat.into_iter().unzip();
                let kind = ExprKind::StructLit { name: name.clone(), type_args: type_args.clone(), fields, spread: spread.clone() };
                return Ok(Some(self.registered(expr, kind, items)));
            }
            _ => Ok(None),
        }
    }

    fn registered(&mut self, expr: &Expr, kind: ExprKind, items: Vec<EntryItems>) -> Expr {
        let rebuilt = Expr { id: expr.id, span: expr.span, kind };
        // The entries sit in the list's own buffer, so their addresses hold
        // when `rebuilt` itself is moved.
        for (e, i) in entry_exprs(&rebuilt).into_iter().zip(items) {
            self.entry_items.insert(e as *const Expr as usize, i);
        }
        rebuilt
    }

    /// Forget the copies `rebuild_spliced` registered.
    pub(crate) fn unscope_rebuilt(&mut self, rebuilt: &Expr) {
        self.unscope_entries(entry_exprs(rebuilt).into_iter());
    }
}

fn unzip(flat: Vec<(CallArg, EntryItems)>) -> (Vec<CallArg>, Vec<EntryItems>) {
    flat.into_iter().unzip()
}

/// The entries of a flattened list, in order.
fn entry_exprs(expr: &Expr) -> Vec<&Expr> {
    match &expr.kind {
        ExprKind::Call { args, .. } | ExprKind::MethodCall { args, .. } => args.iter().map(|a| &a.expr).collect(),
        ExprKind::Array(elems) | ExprKind::Tuple(elems) => elems.iter().collect(),
        ExprKind::StructLit { fields, .. } => fields.iter().map(|f| &f.value).collect(),
        _ => Vec::new(),
    }
}


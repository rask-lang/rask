// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! `comptime for` and `comptime if` among a list's entries (ctrl.comptime/CT69).
//!
//! A list with one is flattened before it's lowered: the loop is unrolled and
//! the `if` decided for this instance, each entry copied once per item it was
//! written under, and a computed label worked out (CT71). The list is then
//! lowered by the same code as a list written out by hand.
//!
//! An entry can still name the loop's item (`f.name`, `v.(f.name)`), and the
//! item differs per copy, so each copy carries the items it was written under.
//! `lower_expr` puts them back in scope while it lowers that copy, the way the
//! statement form lowers its body with the item pushed. The copies are told
//! apart by address: they share their node ids with the one entry written.

use rask_ast::expr::{CallArg, Expr, ExprKind, FieldInit, Splice};
use rask_ast::stmt::ForBinding;

use super::{ComptimeItem, LoweringError, MirLowerer};

/// The loop items in scope where an entry was written, innermost last.
pub(super) type EntryItems = Vec<(String, ComptimeItem)>;

impl<'a> MirLowerer<'a> {
    /// Does this list have entries a `comptime for` or `comptime if` gives?
    pub(super) fn is_spliced<'e>(mut exprs: impl Iterator<Item = &'e Expr>) -> bool {
        exprs.any(|e| matches!(e.kind, ExprKind::Splice(_)))
    }

    /// The entries this instance's list has, each with the items it was
    /// written under. Register them with `scope_entries` once they're where
    /// they'll stay.
    pub(super) fn flatten_entries(&mut self, entries: &[CallArg]) -> Result<Vec<(CallArg, EntryItems)>, LoweringError> {
        let mut out = Vec::new();
        self.flatten_into(entries, &mut out)?;
        Ok(out)
    }

    fn flatten_into(&mut self, entries: &[CallArg], out: &mut Vec<(CallArg, EntryItems)>) -> Result<(), LoweringError> {
        for entry in entries {
            let ExprKind::Splice(splice) = &entry.expr.kind else {
                let mut entry = entry.clone();
                if let Some(label) = entry.computed_label.take() {
                    let name = self.comptime_field_name(&label)?.ok_or_else(|| {
                        LoweringError::InvalidConstruct(
                            "a computed label has to name a field the compiler knows here (ctrl.comptime/CT71)".into(),
                        )
                    })?;
                    entry.name = Some(name);
                }
                out.push((entry, self.comptime_for_bindings.clone()));
                continue;
            };
            match &**splice {
                Splice::For { binding, iter, entries } => {
                    let ForBinding::Single(name) = binding else {
                        return Err(LoweringError::InvalidConstruct(
                            "comptime for requires a single binding — the iterable is a list of fields, not tuples".into(),
                        ));
                    };
                    for item in self.comptime_for_items(iter)? {
                        self.comptime_for_bindings.push((name.clone(), item));
                        let result = self.flatten_into(entries, out);
                        self.comptime_for_bindings.pop();
                        result?;
                    }
                }
                Splice::If { cond, then_entries, else_entries } => {
                    let taken = self.decide_comptime(cond).ok_or_else(|| {
                        LoweringError::InvalidConstruct(
                            "a `comptime if` among a list's entries needs a condition the compiler can settle here (ctrl.comptime/CT69)".into(),
                        )
                    })?;
                    self.flatten_into(if taken { then_entries } else { else_entries }, out)?;
                }
            }
        }
        Ok(())
    }

    /// Remember which items each copy is lowered under. The copies must not
    /// move after this: they're found by address.
    pub(super) fn scope_entries<'e>(&mut self, scoped: impl Iterator<Item = (&'e Expr, &'e EntryItems)>) {
        for (expr, items) in scoped {
            self.entry_items.insert(expr as *const Expr as usize, items.clone());
        }
    }

    /// Forget them again, once the list is lowered.
    pub(super) fn unscope_entries<'e>(&mut self, exprs: impl Iterator<Item = &'e Expr>) {
        for expr in exprs {
            self.entry_items.remove(&(expr as *const Expr as usize));
        }
    }

    /// The items `expr` was written under, when it is a copy a list made.
    pub(super) fn entry_scope(&self, expr: &Expr) -> Option<EntryItems> {
        self.entry_items.get(&(expr as *const Expr as usize)).cloned()
    }

    /// Lower `expr` with `items` as the loop items in scope. A binding hides
    /// an outer local of its name, as in the statement form.
    pub(super) fn with_entry_items<T>(
        &mut self,
        items: EntryItems,
        lower: impl FnOnce(&mut Self) -> Result<T, LoweringError>,
    ) -> Result<T, LoweringError> {
        let hidden: Vec<_> = items
            .iter()
            .filter_map(|(name, _)| self.locals.remove(name).map(|l| (name.clone(), l)))
            .collect();
        let saved = std::mem::replace(&mut self.comptime_for_bindings, items);
        let result = lower(self);
        self.comptime_for_bindings = saved;
        for (name, local) in hidden {
            self.locals.insert(name, local);
        }
        result
    }

    /// A struct literal's fields, as the call it was written as. The field a
    /// `comptime for` fills is named by its label.
    pub(super) fn flatten_fields(&mut self, fields: &[FieldInit]) -> Result<Vec<(FieldInit, EntryItems)>, LoweringError> {
        let as_args: Vec<CallArg> = fields
            .iter()
            .map(|f| match &f.value.kind {
                ExprKind::Splice(_) => CallArg::plain(f.value.clone()),
                _ => CallArg { name: Some(f.name.clone()), ..CallArg::plain(f.value.clone()) },
            })
            .collect();
        self.flatten_entries(&as_args)?
            .into_iter()
            .map(|(arg, items)| {
                let name = arg.name.clone().ok_or_else(|| {
                    LoweringError::InvalidConstruct(
                        "a struct literal's `comptime for` gives fields by name: `(f.name): value` (ctrl.comptime/CT71)".into(),
                    )
                })?;
                Ok((FieldInit { name, value: arg.expr }, items))
            })
            .collect()
    }
}

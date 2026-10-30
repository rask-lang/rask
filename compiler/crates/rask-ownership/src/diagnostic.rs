// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! How the ownership checker's errors read to the user.

use rask_diagnostics::{Diagnostic, ToDiagnostic};

/// Using a `Link<T>` after its node was deleted.
///
/// The move checker is what proves this, but nothing moved: `delete` freed the
/// node, so every name for it is dead. Saying "moved" here would be wrong, and
/// the generic advice — "add `.clone()`" — would hand back a second dead pointer.
fn link_deleted_diagnostic(
    name: &str,
    deleted_at: rask_ast::Span,
    use_span: rask_ast::Span,
    maybe: bool,
) -> Diagnostic {
    let headline = if maybe {
        format!("`{}` may name a deleted node — possible use after free", name)
    } else {
        format!("`{}` names a deleted node — this is a use after free", name)
    };
    let primary = if maybe {
        format!("`{}` is dead on at least one path reaching here", name)
    } else {
        format!("`{}` points at freed memory from here on", name)
    };
    Diagnostic::error(headline)
        .with_code("E0328")
        .with_primary(use_span, primary)
        .with_secondary(deleted_at, format!("the node `{}` names was deleted here", name))
        .with_help("read what you need before the delete, or keep the reference in a field so the rack can null it")
        .with_fix("move the reads above the delete, or rack the link in a `Link<T>?` field")
        .with_why("a `Link<T>` is a pointer to a node, and `delete` frees the node — so every name for it dies at once. A field can survive, because the rack nulls it and the `?` makes you check; a local can't be reached by the rack, so the compiler proves here that you never follow one")
}

impl ToDiagnostic for crate::OwnershipError {
    fn to_diagnostic(&self) -> Diagnostic {
        use crate::OwnershipErrorKind::*;

        match &self.kind {
            UseAfterMove { name, moved_at, reason }
                if matches!(reason, crate::MoveReason::LinkDeleted) =>
            {
                link_deleted_diagnostic(name, *moved_at, self.span, false)
            }

            UseAfterMaybeMove { name, moved_at, reason }
                if matches!(reason, crate::MoveReason::LinkDeleted) =>
            {
                link_deleted_diagnostic(name, *moved_at, self.span, true)
            }

            MutableCaptureConflict { name, holder, captured_at, second_closure } => {
                let primary = if *second_closure {
                    format!("`{}` can't be captured again while `{}` holds it", name, holder)
                } else {
                    format!("`{}` is reached here, and `{}` is still holding it", name, holder)
                };
                Diagnostic::error(format!(
                    "`{}` is already captured for writing by `{}`",
                    name, holder
                ))
                .with_code("E0878")
                .with_primary(self.span, primary)
                .with_secondary(*captured_at, format!("`{}` writes `{}`, so it holds it exclusively", holder, name))
                .with_fix(format!(
                    "move this below `{}`'s last call, or put the variable in a box \
                     both can share: `let {} = Shared.new(…)`, then \
                     `with {}.write() as v {{ … }}` in each closure",
                    holder, name, name
                ))
                .with_why(
                    "a closure that writes a captured variable holds the only \
                     name for it while it lives — that is what makes the write \
                     safe without a lock. A second closure, or a read in \
                     between two calls, is a second name for the same storage, \
                     and which of them wins depends on the order they run in. \
                     The capture ends at the closure's last use, so reading the \
                     variable after that is fine — seeing the mutations is the \
                     point [mem.closures/MC2, MC4]"
                        .to_string(),
                )
            }

            MutableFieldView { binding, path, field_ty } => {
                Diagnostic::error(format!(
                    "`{}` and `{}` would be the same `{}`, both writable",
                    binding, path, field_ty
                ))
                .with_code("E0873")
                .with_primary(self.span, "a field read is a view, not a copy")
                .with_fix(format!(
                    "work through the field — `{}.push(…)` reaches the same storage \
                     without a second name — or ask for a separate value with \
                     `mut {} = {}.clone()`",
                    path, binding, path
                ))
                .with_why(
                    "reading a field gives a view that lives until the block ends, \
                     and a read-only one sits happily beside the source. `mut` asks \
                     for exclusive access instead, and a plain binding can't say for \
                     how long — so both names stay live and a write through either \
                     is a write through both [mem.borrowing/S1, S5]"
                        .to_string(),
                )
            }

            FieldViewStored { into, path, root, field_ty, bound } => {
                use crate::Sink;
                // The value as written at the store: the field read, or the
                // name that holds the view.
                let shown = bound.as_ref().map(|(n, _)| n.as_str()).unwrap_or(path);
                let (headline, owner) = match into {
                    Sink::Return => (
                        format!("returning `{}` would give it a second owner", path),
                        "the caller".to_string(),
                    ),
                    Sink::Place { place: target, .. } => (
                        format!("`{}` and `{}` would be the same `{}`", target, path, field_ty),
                        format!("`{}`", target),
                    ),
                    Sink::StructField { ty, field } => (
                        format!("the new `{}` would share its `{}` with `{}`", ty, field, path),
                        format!("the new `{}`", ty),
                    ),
                    Sink::Element => (
                        format!("`{}` stored in a tuple or array would get a second owner", path),
                        "the tuple or array".to_string(),
                    ),
                    Sink::Payload { variant } => (
                        format!("`{}` would share its payload with `{}`", variant, path),
                        format!("`{}`", variant),
                    ),
                    Sink::Heap => (
                        format!("`Heap({})` would be a second owner of `{}`", shown, path),
                        "the `Heap`".to_string(),
                    ),
                    Sink::TakeArg { callee } => (
                        format!("`{}` takes `{}`, which `{}` still holds", callee, path, root),
                        format!("`{}`", callee),
                    ),
                };
                let fix = match bound {
                    Some((name, _)) => format!(
                        "bind a separate value: `let {} = {}.clone()`",
                        name, path
                    ),
                    None => match into {
                        Sink::Place { place: target, .. } => format!(
                            "store a separate value: `{} = {}.clone()`",
                            target, path
                        ),
                        _ => format!("use a separate value: `{}.clone()`", path),
                    },
                };
                let mut d = Diagnostic::error(headline)
                    .with_code("E0909")
                    .with_primary(
                        self.span,
                        match bound {
                            Some((name, _)) => format!("`{}` is a view of `{}`, not a copy", name, path),
                            None => "a field read is a view, not a copy".to_string(),
                        },
                    );
                if let Some((name, at)) = bound {
                    d = d.with_secondary(*at, format!("`{}` views `{}` from here", name, path));
                }
                d.with_fix(fix).with_why(format!(
                    "reading a field gives a view of storage `{}` still holds, \
                     and {} would own it too: one value with two owners, so a \
                     write through either changes both and each frees it \
                     [mem.borrowing/S1, S3]",
                    root,
                    owner
                ))
            }

            BorrowedFieldEscapes { path, root, field_ty, declared_at, is_mutate, of_closure } => {
                let mode = if *is_mutate { "`mutate` borrow" } else { "borrow" };
                let fix = if *of_closure {
                    format!(
                        "return a copy — `{}.clone()` — or take the parameter: `|take {}: …|`",
                        path, root
                    )
                } else {
                    format!(
                        "return a copy — `{}.clone()` — or take the {}: `{}`, \
                         so the call site shows the value going",
                        path,
                        if root == "self" { "receiver" } else { "parameter" },
                        if root == "self" { "take self".to_string() } else { format!("take {}: …", root) }
                    )
                };
                Diagnostic::error(format!(
                    "`{}` belongs to the caller — returning it hands out a second name for it",
                    path
                ))
                .with_code("E0872")
                .with_primary(self.span, format!("`{}` isn't Copy, so this is a view, not a copy", field_ty))
                .with_secondary(*declared_at, format!("`{}` is a {} — the caller keeps it", root, mode))
                .with_fix(fix)
                .with_why(
                    "a parameter without `take` is the caller's value on loan. It, \
                     a field of it, or a payload matched out of it is a view that \
                     lives until the block ends. Handing \
                     that view back leaves the caller and the callee's caller both \
                     holding the same storage: a write through one is a write \
                     through the other, and whoever frees it second frees it twice \
                     [mem.borrowing/S3, mem.parameters/PM1]"
                        .to_string(),
                )
            }

            DropOfAnOwnedField { path, root, field_ty } => {
                Diagnostic::error(format!(
                    "`{}` belongs to `{}` — dropping it here frees it twice",
                    path, root
                ))
                .with_code("E0880")
                .with_primary(
                    self.span,
                    format!("`{}` moved in when it was stored, so `{}` gives it back", field_ty, root),
                )
                .with_fix(format!(
                    "drop the whole thing — `drop({})` — or let it go out of scope, \
                     which releases the field either way",
                    root
                ))
                .with_why(
                    "storing a box in a field moves it in, and the aggregate's \
                     release hands it back when the aggregate dies. A hand-drop of \
                     the same field is a second owner and both of them run — the \
                     drop frees the block and the release frees it again. A `drop` \
                     that quietly did nothing here would be worse: whether it frees \
                     anything would depend on whether its argument is a binding or a \
                     projection, which the line doesn't say \
                     [mem.heap/HP3, mem.linear/L5]"
                        .to_string(),
                )
            }

            LentValueEscapes { into, call, holder, lender, payload_ty, clone_form, lent_at } => {
                use crate::Sink;
                let verb = if matches!(into, Sink::Return) { "hand back" } else { "store" };
                let fix = match clone_form {
                    Some(m) => format!(
                        "{} a copy — `{}` has `{}`, which allocates a new `{}` and says so at the call site",
                        verb, lender, m, payload_ty
                    ),
                    None => format!(
                        "{} a copy — `.clone()` what the lookup answers with, so the new `{}` is visible here",
                        verb, payload_ty
                    ),
                };
                let second = match into {
                    Sink::Return => "returning it gives the caller a second name for it".to_string(),
                    Sink::Place { place, .. } => format!("storing it in `{}` gives it a second owner", place),
                    Sink::StructField { ty, field } => format!("the new `{}`'s `{}` would be a second owner of it", ty, field),
                    Sink::Element => "a tuple or array holding it would be a second owner".to_string(),
                    Sink::Payload { variant } => format!("`{}` holding it would be a second owner", variant),
                    Sink::Heap => "a `Heap` holding it would be a second owner".to_string(),
                    Sink::TakeArg { callee } => format!("`{}` would take it as a second owner", callee),
                };
                let primary = match into {
                    Sink::Return => format!("the signature promises an owned `{}`, and this path doesn't have one", payload_ty),
                    _ => format!("this would own a `{}` that `{}` still holds", payload_ty, holder),
                };
                Diagnostic::error(format!("`{}` hands back what `{}` still holds — {}", call, holder, second))
                .with_code("E0879")
                .with_primary(self.span, primary)
                .with_secondary(*lent_at, format!("`{}` keeps the value; the lookup reads it in place", holder))
                .with_fix(fix)
                .with_why(
                    "a lookup reads the element where the container keeps it, so what \
                     comes back is the container's, not a copy. A `??` makes that \
                     invisible: the default side is fresh and the lookup side isn't, \
                     and one return slot can't be both. Freeing it corrupts the \
                     container and not freeing it leaks, so the copy is the program's \
                     to write [mem.borrowing/S3, mem.parameters/PM1]"
                        .to_string(),
                )
            }

            LentValueGivenAway { call, holder, lender, payload_ty, method, clone_form } => {
                let fix = match clone_form {
                    Some(c) => format!(
                        "copy it with `{holder}.{c}(…)`, which hands back a `{payload_ty}` of your own — \
                         or take it out with `{holder}.remove(…)` if `{holder}` is done with it"
                    ),
                    None => format!(
                        "copy it first: `{call}.clone().{method}()` — the cost of the new `{payload_ty}` shows at the call"
                    ),
                };
                Diagnostic::error(format!(
                    "`{}` lends what `{}` still holds, and `{}` takes it",
                    call, holder, method
                ))
                .with_code("E0906")
                .with_primary(
                    self.span,
                    format!("`{}` takes this `{}`, and it stays in `{}`", method, payload_ty, holder),
                )
                .with_fix(fix)
                .with_why(format!(
                    "a `{lender}` lookup reads the element where the container keeps it, so it isn't yours \
                     to give. A `take` owns what it's handed and frees it or passes it on, while \
                     `{holder}` frees the same value again later [mem.borrowing/S3, mem.parameters/PM1]"
                ))
            }

            NonCopyElementCopiedOut { binding, elem_ty, collection } => {
                let from = collection
                    .as_deref()
                    .map(|c| format!("`{}`", c))
                    .unwrap_or_else(|| "the collection".to_string());
                Diagnostic::error(format!(
                    "`{}` isn't Copy, so `{}` would be a second name for the same element",
                    elem_ty, binding
                ))
                .with_code("E0871")
                .with_primary(
                    self.span,
                    format!("this element stays owned by {}", from),
                )
                .with_fix(format!(
                    "scope the access — `with {}[…] as {} {{ … }}` — or ask for a \
                     separate value with `.clone()`",
                    collection.as_deref().unwrap_or("collection"),
                    binding
                ))
                .with_why(
                    "indexing hands the element back where it lives rather than \
                     copying it out, so a binding of a non-Copy element is a second \
                     name for storage the collection still owns — a write through \
                     either one is a write through both. A Copy element is copied \
                     and has no such tie [mem.borrowing/E4]"
                        .to_string(),
                )
            }

            TakeInGenericInstance { chain, param, type_arg, take_at } => {
                let func = chain.last().map(String::as_str).unwrap_or("?");
                let via = if chain.len() > 1 {
                    format!(" (reached through {})", chain.iter().map(|c| format!("`{}`", c)).collect::<Vec<_>>().join(" → "))
                } else {
                    String::new()
                };
                Diagnostic::error(format!(
                    "`{}` takes a `Shared<{}>` apart, and this call gives it `{}` = `{}`",
                    func, param, param, type_arg
                ))
                .with_code("E0920")
                .with_primary(self.span, format!("`{}` = `{}` here{}", param, type_arg, via))
                .with_secondary(*take_at, format!("`take` empties the box, in `{}`", func))
                .with_fix(format!(
                    "use `replace` in `{}` if any `{}` should work, or call it with a box holding a resource",
                    func, param
                ))
                .with_why("`take` empties the box, which is only sound on the last handle. A `Shared` is single-handle only when its value must be consumed, and the body is checked against each type it's called with [conc.sync/SH9]")
            }

            LinearInGenericInstance { chain, type_args, inner, inner_span } => {
                let func = chain.last().map(String::as_str).unwrap_or("?");
                let what = match inner.as_ref() {
                    ResourceNotConsumed { name, .. }
                    | ResourceNotConsumedOpaque { name, .. }
                    | OwnedNotConsumed { name } => format!("drops `{}` without consuming it", name),
                    LinearWildcardDiscard { .. } | ResourceDiscardedAsStatement { .. } => {
                        "throws a value away with `_`".to_string()
                    }
                    ResourceAlreadyConsumed { name, .. }
                    | UseAfterMove { name, .. }
                    | UseAfterMaybeMove { name, .. } => format!("uses `{}` after giving it away", name),
                    _ => "does something to it a linear value can't take".to_string(),
                };
                let via = if chain.len() > 1 {
                    format!(" (reached through {})", chain.iter().map(|c| format!("`{}`", c)).collect::<Vec<_>>().join(" → "))
                } else {
                    String::new()
                };
                Diagnostic::error(format!(
                    "`{}` {} — with {} that's a linear value",
                    func, what, type_args
                ))
                .with_code("E0904")
                .with_primary(self.span, format!("this call makes it {}{}", type_args, via))
                .with_secondary(*inner_span, format!("here, in `{}`: {}", func, inner))
                .with_fix(format!(
                    "consume it in `{}` on every path (pass it to a `take` parameter, call its consuming method, or return it), or don't pass a linear value here",
                    func
                ))
                .with_why("a generic body is checked again for each type it's called with, and a linear value must be consumed exactly once. Fine for an ordinary `T`, this body isn't for that one [mem.linear/L1, L2, type.generics/G6]")
            }

            SmallInstantiationTooBig { type_name, base_name, size, offending_field } => {
                let label = match offending_field {
                    Some((field, field_size, field_ty)) => format!(
                        "{} bytes at `{}` — `{}` is a `{}` there, {} of them",
                        size, type_name, field, field_ty, field_size
                    ),
                    None => format!("{} bytes at `{}`, and the limit is 16", size, type_name),
                };
                Diagnostic::error(format!(
                    "`@small` type `{}` outgrew the copy threshold at `{}`",
                    base_name, type_name
                ))
                .with_code("E0375")
                .with_primary(self.span, label)
                .with_fix(format!(
                    "don't instantiate `{}` with a type that big — or drop `@small`, \
                     since the fence can't hold for every type argument",
                    base_name
                ))
                .with_why("`@small` is read at the definition but it's a promise about every instantiation. The same source text is 16 bytes at one type argument and 32 at another, and only the second one breaks the promise — so the check runs per instantiation, like any other generic bound [mem.value/SM3, type.generics/G2]")
            }

            SmallTypeTooBig { type_name, size, offending_field } => {
                let label = match offending_field {
                    Some((field, field_size)) => format!(
                        "{} bytes — `{}` is the {}-byte field that took it over 16",
                        size, field, field_size
                    ),
                    None => format!("{} bytes, and the limit is 16", size),
                };
                let fix = match offending_field {
                    Some((field, _)) => format!(
                        "shrink or move out `{}` — or drop `@small` and let the \
                         move errors at the call sites stand",
                        field
                    ),
                    None => "shrink the type — or drop `@small` and let the move \
                             errors at the call sites stand"
                        .to_string(),
                };
                Diagnostic::error(format!(
                    "`@small` type `{}` outgrew the copy threshold",
                    type_name
                ))
                .with_code("E0374")
                .with_primary(self.span, label)
                .with_fix(fix)
                .with_why("`@small` asserts one thing: the type stays within the 16-byte copy threshold (mem.value/SM1). It buys the *location* of the error — without it, growing past 16 bytes flips every assignment from copy to move and those errors land wherever the type is used, with only the move note pointing back at a field nobody was looking at [mem.value/SM2, VS1, VS6]")
            }

            MutateParamLeftEmpty { name, consumed_at, declared_at, maybe } => {
                let label = if *maybe {
                    format!("`{}` is given away on some paths here and not replaced", name)
                } else {
                    format!("`{}` is given away here and not replaced", name)
                };
                let fix = if *maybe {
                    format!("put a value back on every path — or move the `{}` out of the branch", name)
                } else {
                    format!("assign a replacement before returning: `{} = …`", name)
                };
                Diagnostic::error(format!("gave `{}` away and didn't put anything back", name))
                    .with_code("E0836")
                    .with_primary(*consumed_at, label)
                    .with_secondary(*declared_at, format!("`{}` is declared `mutate`", name))
                    .with_fix(fix)
                    .with_why("a `mutate` parameter is exclusive access, not ownership: the caller keeps the value and reads it after the call. Taking it out and writing a replacement back is what the mode is for — leaving the slot empty hands them a hole [mem.parameters/PM2, PM6]".to_string())
            }

            ConsumeConst { name, sink } => {
                let label = match sink {
                    Some(s) => format!("`{}` takes ownership, and a const isn't anyone's to give", s),
                    None => "this takes ownership, and a const isn't anyone's to give".to_string(),
                };
                Diagnostic::error(format!("cannot give away `{}` — it's a const", name))
                    .with_code("E0883")
                    .with_primary(self.span, label)
                    .with_fix(format!("hand over a copy: `{}.clone()`", name))
                    .with_why(
                        "every function reads the same const, so giving it away here would leave the rest holding nothing. The clone is written because it allocates [mem.ownership/O11]".to_string()
                    )
            }

            ToVecOfLentItems { elem, adapter } => {
                Diagnostic::error(format!("`to_vec` has nothing it may move — `{}` is lent, not owned", elem))
                    .with_code("E0905")
                    .with_primary(self.span, "needs an owned element")
                    .with_fix("clone where you mean it — a `map` makes values the chain owns: `.map(|x| x.clone()).to_vec()`")
                    .with_why(format!(
                        "`{}` hands on what its source lent it, so every item still belongs to the source. A Vec owns what it holds, and `to_vec` won't deep-clone a `{}` on your behalf — the allocation would be invisible [type.sequence/SEQ47]",
                        adapter, elem
                    ))
            }

            ZipOfLentItems { elem } => {
                Diagnostic::error(format!("`zip` can't pair `{}` — both vectors only lend their elements", elem))
                    .with_code("E0924")
                    .with_primary(self.span, "the pairs would need elements of their own")
                    .with_fix("pair by index and clone where you mean it: `for i in 0..a.len() { pairs.push((a[i].clone(), b[i])) }`")
                    .with_why(format!(
                        "`zip` borrows both vectors and builds pairs that own what they hold. A copy can give a pair its own Copy value, and a `{}` isn't one; `zip` won't deep-clone it on your behalf, because the allocation would be invisible [type.sequence/SEQ47]",
                        elem
                    ))
            }

            ConsumeBorrowedParam { name, declared_at, is_mutate, sink, of_closure } => {
                let how = if *is_mutate { "`mutate` parameter" } else { "borrowed parameter" };
                let label = match sink {
                    Some(s) => format!("`{}` takes ownership, and `{}` isn't yours to give", s, name),
                    None => format!("this takes ownership, and `{}` isn't yours to give", name),
                };
                let mutate_note = if *is_mutate {
                    " `mutate` is exclusive access — you may write through it, not give it away."
                } else {
                    ""
                };
                Diagnostic::error(format!("cannot give away `{}` — it's borrowed, not owned", name))
                    .with_code("E0835")
                    .with_primary(self.span, label)
                    .with_secondary(*declared_at, format!("`{}` is declared as a {}", name, how))
                    .with_fix(if *of_closure {
                        format!("take it: `|take {}: …|` — then the caller can see it goes", name)
                    } else {
                        format!("take it: `take {}: …` in the signature — then the caller can see it goes", name)
                    })
                    .with_why(format!(
                        "the caller keeps a parameter it didn't mark `take` and goes on using it, so consuming it here would leave them holding something that's gone. For a `@resource` that's a second close of a real handle.{} [mem.parameters/PM1, mem.linear/L1]",
                        mutate_note
                    ))
            }

            UseAfterMove { name, moved_at, reason } => {
                use crate::MoveReason;
                let (note, help) = match reason {
                    MoveReason::SizeExceedsThreshold { type_name, size } => (
                        format!(
                            "`{}` is {} bytes (copy threshold is 16) — assignment moves instead of copying",
                            type_name, size
                        ),
                        format!(
                            "add `{}.clone()` if you need an independent copy",
                            name
                        ),
                    ),
                    MoveReason::OwnsHeapMemory { type_name } => (
                        format!(
                            "`{}` owns heap memory — assignment moves instead of copying",
                            type_name
                        ),
                        format!(
                            "add `{}.clone()` if you need an independent copy",
                            name
                        ),
                    ),
                    MoveReason::Unique { type_name } => (
                        format!("`{}` is @unique — implicit copy is disabled", type_name),
                        format!(
                            "add `{}.clone()` if the type supports it",
                            name
                        ),
                    ),
                    MoveReason::Resource { type_name } => (
                        format!(
                            "`{}` is a resource: it's consumed exactly once, and it can't be copied or cloned",
                            type_name
                        ),
                        format!("use `{}` before the line that consumes it, or not at all", name),
                    ),
                    MoveReason::LinkDeleted => (
                        // Unreachable — the guarded arm above handles it.
                        format!("`{}` names a deleted node", name),
                        "read the node before deleting it".to_string(),
                    ),
                    MoveReason::LinkMoved => (
                        format!(
                            "a `Link<T>` moves like any other name for a node — `{}` handed its node over rather than copying it",
                            name
                        ),
                        "read through the new name, or keep the edge in a `Link<T>?` field where the rack maintains it".to_string(),
                    ),
                    MoveReason::Owned => (
                        format!(
                            "`{}` is a Heap box — it was consumed there, and its \
                             memory went with it",
                            name
                        ),
                        "consume it once. If two owners are really needed, clone the \
                         value into a second box"
                            .to_string(),
                    ),
                    MoveReason::Unknown => (
                        format!("`{}` was moved — assignment transfers ownership", name),
                        format!(
                            "add `{}.clone()` if you need a separate copy",
                            name
                        ),
                    ),
                };
                Diagnostic::error(format!("use of moved value: `{}`", name))
                    .with_code("E0800")
                    .with_primary(self.span, "value used here after move")
                    .with_secondary(*moved_at, "value moved here")
                    .with_note(note)
                    .with_help(help)
            }

            UseAfterMaybeMove { name, moved_at, reason } => {
                use crate::MoveReason;
                let note = match reason {
                    MoveReason::Resource { type_name } => format!(
                        "`{}` is @resource — moved on one branch but not the other",
                        type_name
                    ),
                    MoveReason::Owned => format!(
                        "`{}` is a Heap box — consumed on one branch but not the other, \
                         and after the branches join the compiler has to assume it went",
                        name
                    ),
                    _ => format!(
                        "`{}` is moved on one branch but not the other — after the branches \
                         join the compiler must assume it was moved",
                        name
                    ),
                };
                Diagnostic::error(format!("use of maybe-moved value: `{}`", name))
                    .with_code("E0813")
                    .with_primary(self.span, "value used here, but it may have been moved")
                    .with_secondary(*moved_at, "value moved here on one path")
                    .with_note(note)
                    .with_help(format!(
                        "move `{}` on every path, or restructure so the use happens inside the \
                         branch that still owns it",
                        name
                    ))
            }

            BorrowConflict {
                name,
                requested,
                existing,
                existing_span,
            } => {
                let fix_msg = match (requested.participle(), existing.participle()) {
                    ("written to", "read") => {
                        "finish reading before you write, or read from a `.clone()` so the write has the original to itself"
                    }
                    _ => "restructure the code to avoid conflicting access",
                };
                Diagnostic::error(format!(
                    "cannot {} `{}` while it is being {}",
                    requested.verb(), name, existing.participle()
                ))
                .with_code("E0801")
                .with_primary(self.span, format!("{} access here", requested.noun()))
                .with_secondary(*existing_span, format!("{} access here", existing.noun()))
                .with_help(fix_msg)
                .with_fix(fix_msg)
                .with_why("concurrent read and write access to the same value would be a data race")
            }

            MoveFromBorrowedParam { name } => {
                Diagnostic::error(format!(
                    "cannot move `{}` — parameter is borrowed, not owned",
                    name
                ))
                .with_code("E0806")
                .with_primary(self.span, "move occurs here")
                .with_fix(format!("use `take {}` in the parameter list to transfer ownership", name))
                .with_why("borrowed parameters can only be read — the caller retains ownership")
            }

            ResourceAlreadyConsumed { name, consumed_at } => {
                Diagnostic::error(format!("resource `{}` already consumed", name))
                    .with_code("E0807")
                    .with_primary(self.span, "second use here")
                    .with_secondary(*consumed_at, "resource consumed here")
                    .with_why("resources must be consumed exactly once")
            }

            MutateWhileBorrowed { name, borrow_span } => {
                Diagnostic::error(format!(
                    "`{}` cannot be changed while it's being read",
                    name
                ))
                .with_code("E0802")
                .with_primary(self.span, "mutation occurs here")
                .with_secondary(*borrow_span, "borrow is active here")
                .with_help(format!("restructure so the borrow ends before mutating, or use `{}.clone()` to work on an independent copy", name))
                .with_why("mutation during an active borrow could invalidate the borrow's view of the data")
            }

            InstantBorrowEscapes { source_type } => {
                Diagnostic::error(format!(
                    "cannot store reference from `{}`",
                    source_type
                ))
                .with_code("E0803")
                .with_primary(self.span, "reference would escape")
                .with_help("use the value inline or copy it out")
                .with_fix("use the value inline or copy it out")
                .with_why("collection element references are expression-scoped — they can't outlive the access expression")
            }

            BorrowEscapes { name } => {
                Diagnostic::error(format!(
                    "`{}` would become invalid after this point",
                    name
                ))
                .with_code("E0804")
                .with_primary(self.span, "borrow would escape scope")
                .with_help("ensure the value lives long enough, or clone it")
                .with_fix("ensure the value lives long enough, or clone it")
                .with_why("references cannot outlive their source — Rask prevents dangling references by construction")
            }

            MutateParamNotReplaced { name, ty, consumed_at } => {
                let ctor = if ty.is_empty() {
                    "a new one".to_string()
                } else {
                    format!("`{}.new(…)`", ty)
                };
                Diagnostic::error(format!(
                    "`{}` was consumed and never put back",
                    name
                ))
                .with_code("E0860")
                .with_primary(self.span, format!("`{}` is still empty when this returns", name))
                .with_secondary(*consumed_at, format!("`{}` was consumed here", name))
                .with_help(format!(
                    "assign {} to `{}` before returning, or declare it `take {}` and return the replacement",
                    ctor, name, name
                ))
                .with_fix(format!("{} = {}", name, ctor))
                .with_why("`mutate` lends the value and takes it back — the caller keeps using the same binding afterwards. Consuming it is allowed, because consume-and-replace is a real pattern, but the slot has to hold something again by the time control leaves [mem.parameters/PM3]")
            }

            ConsumedBorrowedParam { name, ty } => {
                let decl = if ty.is_empty() {
                    format!("take {}", name)
                } else {
                    format!("take {}: {}", name, ty)
                };
                Diagnostic::error(format!(
                    "`{}` is borrowed from the caller — it can't be given away here",
                    name
                ))
                .with_code("E0806")
                .with_primary(self.span, format!("`{}` is consumed here", name))
                .with_help(format!(
                    "declare it `{}` if this function should own it, or read what you need instead of passing it on",
                    decl
                ))
                .with_fix(decl)
                .with_why("a parameter without `take` is a borrow: the caller keeps the value and goes on using it. Consuming it here would consume it twice — for a `@resource` that means the cleanup runs twice, which mem.linear/L1 exists to make impossible at compile time [mem.parameters/PM3, mem.linear/L1]")
            }

            LocalSharedSent { name } => {
                Diagnostic::error("this `Shared` is task-local and cannot be sent")
                    .with_code("E0346")
                    .with_primary(self.span, format!("`{}` uses the `Local` strategy", name))
                    .with_fix(
                        "drop the `.local` — `Shared.new(…)` locks, and `Shared.mutex(…)` \
                         locks more cheaply when writes dominate"
                    )
                    .with_why(
                        "`Local` takes no lock at all, so two tasks touching it would \
                         race. It is the opt-out, not the default, and this error is \
                         what makes it safe to reach for [conc.sync/SH7]",
                    )
            }
            LinkSent { name, ty, bare_link } => {
                let label = if *bare_link {
                    format!("`{}` is a link into a rack", name)
                } else {
                    format!("`{}` is a `{}`, which holds a link", name, ty)
                };
                Diagnostic::error(format!("`{}` holds a link, which can't go to another task", name))
                    .with_code("E0901")
                    .with_primary(self.span, label)
                    .with_fix(
                        "copy out what the task needs before the spawn — `let id = n.id` — \
                         or hand it the whole graph: `let copy = rack.snapshot()` and use \
                         `copy` inside",
                    )
                    .with_why(
                        "a link is its node's address, and every link can write the node, \
                         so two tasks holding one would both write the same memory with \
                         nothing ordering them. A rack moves as a whole, links and all; a \
                         link on its own never crosses [mem.ownership/T2]",
                    )
            }
            LinkOutlivesRack { link, rack, via, carried } => {
                let holds = if *carried { "holds links into" } else { "lives in" };
                let (primary, fix) = match via {
                    crate::LinkEscape::Return => (
                        format!(
                            "`{}` {} `{}`, and `{}` dies when this function returns",
                            link, holds, rack, rack
                        ),
                        if *carried {
                            format!(
                                "return the nodes' data instead, or take the rack as a parameter so it outlives the call: `func …(mutate {}: Rack<…>) -> Vec<Link<…>>`",
                                rack
                            )
                        } else {
                            format!(
                                "return the node's data instead, or take the rack as a parameter so it outlives the call: `func …(mutate {}: Rack<…>) -> Link<…>`",
                                rack
                            )
                        },
                    ),
                    crate::LinkEscape::Assignment { target } => (
                        format!(
                            "`{}` outlives `{}`, and the node{} `{}` points at dies with it",
                            target, rack, if *carried { "s" } else { "" }, link
                        ),
                        format!(
                            "move `{}` out to where `{}` lives, or copy the fields you need out of the node before the scope ends",
                            rack, target
                        ),
                    ),
                };
                let headline = if *carried {
                    format!("`{}` holds links that would outlive the rack they point into", link)
                } else {
                    format!("`{}` would outlive the rack it points into", link)
                };
                Diagnostic::error(headline)
                    .with_code("E0379")
                    .with_primary(self.span, primary)
                    .with_fix(fix)
                    .with_why("a `Link<T>` is a pointer to a node, and the nodes live in the rack — so when the rack goes out of scope the node goes with it and the link dangles. Nothing else catches this: no `delete` happened, so the use-after-delete rule never looks, and a link is Copy so it escapes the scope that produced it. A link into a rack the *caller* owns is fine — that rack outlives the call")
            }
            NodeWriteNeedsWritableRack { link, rack } => {
                // `with_help` isn't rendered on this path — `fix` and `why` are —
                // so the actionable line goes in `with_fix`.
                let d = match rack {
                    Some(s) => Diagnostic::error(format!(
                        "cannot write this node — `{}` is only readable here",
                        s
                    ))
                    .with_code("E0378")
                    .with_primary(
                        self.span,
                        format!("writes a node in `{}` through `{}`", s, link),
                    )
                    .with_fix(format!(
                        "make the rack writable: `mut {}` if it's a local, `mutate {}: Rack<…>` if it's a parameter",
                        s, s
                    )),
                    None => Diagnostic::error(format!(
                        "cannot write this node — `{}` is a view",
                        link
                    ))
                    .with_code("E0378")
                    .with_primary(
                        self.span,
                        format!("`{}` was lent for reading, not for writing", link),
                    )
                    .with_fix(format!(
                        "say the function writes it: `mutate {}: Link<…>` in the signature — the caller then marks it `mutate {}` at the call",
                        link, link
                    )),
                };
                d.with_why("writing a node is a permission, and permission travels with the link: `n: Link<T>` is a view — it reads the node and everything reachable from it — and `mutate n: Link<T>` is the writable one. It's the same borrow-versus-mutate distinction every other type has, so no second link type is needed and no rack has to be threaded through. A view stays a view when you follow an edge, and can't be passed on as `mutate`; a writer's permission does travel along edges, because an edge only connects nodes in one rack")
            }
            UndeclaredDelete { param, operation } => {
                Diagnostic::error(format!(
                    "this can delete nodes the caller never named — declare `deleting {}`",
                    param
                ))
                .with_code("E0329")
                .with_primary(self.span, format!("{} chooses which nodes die", operation))
                .with_help(format!(
                    "declare it: `deleting {}: Rack<…>` — or delete only links the caller handed over, as `take` parameters",
                    param
                ))
                .with_fix(format!("deleting {}", param))
                .with_why("`delete(take link)` is safe for the caller because the link is consumed at the call site — they can see the name die. A delete that chooses its own victim can't be seen from outside, so the caller's links are revoked at the call instead, and `deleting` is what tells them to expect it. `mutate` doesn't imply it: inserting and writing can't invalidate a link, deleting can")
            }

            ResourceNotConsumedOpaque { name, where_ } => {
                Diagnostic::error(format!(
                    "resource `{}` must be consumed before scope exit",
                    name
                ))
                .with_code("E0805")
                .with_primary(
                    self.span,
                    format!("scope ends after this, and `{}` is still owed", name),
                )
                .with_help(format!(
                    "consume the resource inside `{}`, then `discard {}` — or hold it \
                     somewhere the compiler can name, like a plain field",
                    name, name
                ))
                .with_fix(format!(
                    "take the resource out of `{}` and consume it",
                    name
                ))
                .with_note(format!(
                    "the resource sits in {} — there is no field path to it, so the whole of `{}` is owed rather than one field",
                    where_, name
                ))
                .with_why("resource types must be explicitly consumed — this prevents resource leaks")
            }

            ResourceNotConsumed { name, consume } => {
                let fix = match consume.as_slice() {
                    [] => format!(
                        "pass `{}` to something that takes it, or `ensure` a call that does",
                        name
                    ),
                    [c] => format!(
                        "call `{0}` before the block ends, or `ensure {0}` right after acquiring it",
                        c
                    ),
                    calls => format!(
                        "call {} before the block ends, or `ensure` one of them right after acquiring it",
                        either(calls, "")
                    ),
                };
                Diagnostic::error(format!(
                    "resource `{}` must be consumed before scope exit",
                    name
                ))
                .with_code("E0805")
                // The span is the block's last statement, which is where the
                // scope ends — not always where the resource is. On an `if`
                // that consumes on one branch only, "resource goes out of scope
                // here" pointed at the `if` and read as nonsense.
                .with_primary(
                    self.span,
                    format!("scope ends after this, and `{}` is still owed", name),
                )
                .with_fix(fix)
                .with_why("resource types must be explicitly consumed — this prevents resource leaks")
            }

            ResourceLeaksOnTry { name, acquired_at, consume } => {
                Diagnostic::error(format!(
                    "`{}` would leak if this fails",
                    name
                ))
                .with_code("E0881")
                .with_primary(
                    self.span,
                    format!("the error leaves the function here, with `{}` still open", name),
                )
                .with_secondary(*acquired_at, format!("`{}` was acquired here", name))
                .with_fix(format!(
                    "add {} on the line after it's acquired",
                    ensure_call(name, consume)
                ))
                .with_why(
                    "a resource has to be consumed on every way out of the scope, and `try` is one of them",
                )
            }

            ResourceCommitDeferred { name, acquired_at, consume } => {
                Diagnostic::error(format!("`{}` has no cleanup committed yet", name))
                    .with_code("E0882")
                    .with_primary(
                        self.span,
                        format!("this runs while `{}` has nothing scheduled to close it", name),
                    )
                    .with_secondary(*acquired_at, format!("`{}` was acquired here", name))
                    .with_fix(format!(
                        "add {} on the line after it's acquired, then use `{}` freely",
                        ensure_call(name, consume),
                        name
                    ))
                    .with_why(
                        "a panic here would leak it — nothing is scheduled to clean it up, and there are no destructors to fall back on",
                    )
            }

            EnsureConsumesNothing { name, acquired_at, consume } => {
                Diagnostic::error(format!("nothing in this `ensure` consumes `{}`", name))
                    .with_code("E0908")
                    .with_primary(
                        self.span,
                        format!("this doesn't consume `{}`, so nothing closes it at scope exit", name),
                    )
                    .with_secondary(*acquired_at, format!("`{}` was acquired here", name))
                    .with_fix(format!("call a method that takes `{}`: {}", name, ensure_call(name, consume)))
                    .with_why(
                        "`ensure` commits the cleanup its body runs, and a call that only borrows the resource is no cleanup",
                    )
            }

            ResourceDiscardedAsStatement { type_name } => {
                Diagnostic::error(format!(
                    "value of resource type `{}` is dropped without being consumed",
                    type_name
                ))
                .with_code("E0840")
                .with_primary(self.span, "produced here and immediately dropped")
                .with_help("bind it to a name and call a consuming method on it, e.g. `let h = ...; h.join()`")
                .with_fix("bind the value to a name so it can be consumed")
                .with_why("resource types must be explicitly consumed — this prevents resource leaks")
            }

            OwnedNotConsumed { name } => {
                Diagnostic::error(format!(
                    "`{}` was allocated with `Heap(…)` and never dropped",
                    name
                ))
                .with_code("E0837")
                .with_primary(self.span, "the value goes out of scope here, still owned")
                .with_help(format!(
                    "consume it exactly once: `drop({})`, hand it to a `take` parameter, \
                     store it in a field, or return it",
                    name
                ))
                .with_fix(format!("drop({})", name))
                .with_why(
                    "a Heap value has one owner and must be consumed exactly once \
                     (mem.linear/L1) — nothing else frees it",
                )
            }

            ResourceNotConsumedInClosure { name, in_task: true } => {
                Diagnostic::error(format!(
                    "resource `{}` is not consumed on every path through the task",
                    name
                ))
                .with_code("E0810")
                .with_primary(self.span, format!("this task captures `{}` and can end without consuming it", name))
                .with_fix(format!("consume it in the task — `ensure {}.close()` at the top of the block", name))
                .with_why("a task carries away what it captures, so the parent can't close it any more: the task owns it, and a resource is consumed exactly once [conc.async/S6, mem.linear/L1]")
            }

            ResourceNotConsumedInClosure { name, in_task: false } => {
                Diagnostic::error(format!(
                    "resource `{}` is not consumed on every path through the closure",
                    name
                ))
                .with_code("E0810")
                .with_primary(self.span, format!("closure body ends without consuming `{}`", name))
                .with_fix(format!("ensure {}.close()", name))
                .with_why("the closure owns it — a `take` parameter or something the body acquired — and a resource is consumed exactly once [mem.linear/L1]")
            }

            LinearCaptureCarried { name, ty } => {
                let ty = ty.clone().unwrap_or_else(|| "T".to_string());
                Diagnostic::error(format!(
                    "`{}` must be consumed, and a closure can't consume what it captures",
                    name
                ))
                .with_code("E0913")
                .with_primary(self.span, format!("this closure outlives its frame, so it would carry `{}` away and never let go of it", name))
                .with_fix(format!(
                    "pass it in as an argument the closure takes — a `take` parameter its \
                     caller fills:\n\n  \
                     |take {n}: {t}| {{ … }}\n\n\
                     or, if this runs once as a task, a task block, which may consume what \
                     it captures:\n\n  \
                     spawn {{ … }}",
                    n = name, t = ty
                ))
                .with_why(
                    "a closure may run any number of times, so its body can't give away \
                     what it captured [mem.closures/CM4]. One that outlives its frame moves \
                     its captures in [CM2], and a linear value moved there could never be \
                     consumed. A `take` parameter is a fresh value per call, so consuming it is fine",
                )
            }

            ConsumeBorrowedPart { name, from, matched_at, sink } => {
                let label = match sink {
                    Some(s) => format!("`{}` takes ownership, and `{}` isn't yours to give", s, name),
                    None => format!("this takes ownership, and `{}` isn't yours to give", name),
                };
                Diagnostic::error(format!(
                    "cannot give away `{}` — it's part of `{}`, which is borrowed",
                    name, from
                ))
                .with_code("E0899")
                .with_primary(self.span, label)
                .with_secondary(*matched_at, format!("`{}` comes out of `{}` here", name, from))
                .with_fix(format!("take `{}` in the signature: `take {}: …`", from, from))
                .with_why(format!(
                    "matching a borrowed value doesn't take it apart — the caller still holds `{}`, `{}` included, so giving `{}` away would leave them holding something that's gone. [mem.parameters/PM1, mem.linear/L1]",
                    from, name, name
                ))
            }

            ConsumeLoopItem { name, from, loop_at, sink } => {
                let label = match sink {
                    Some(s) => format!("`{}` takes ownership, and `{}` is still in `{}`", s, name, from),
                    None => format!("this takes ownership, and `{}` is still in `{}`", name, from),
                };
                Diagnostic::error(format!(
                    "cannot give away `{}` — the loop only lends it out of `{}`",
                    name, from
                ))
                .with_code("E0902")
                .with_primary(self.span, label)
                .with_secondary(*loop_at, format!("each `{}` is borrowed from `{}`", name, from))
                .with_fix(format!("{}.clone()", name))
                .with_why(format!(
                    "a `for` loop lends each element and `{}` keeps it, so giving `{}` away would leave two owners of one value. [ctrl.loops/LP6]",
                    from, name
                ))
            }

            ConsumeWithBinding { name, from, with_at, sink } => {
                let label = match sink {
                    Some(s) => format!("`{}` takes ownership, and `{}` is still in `{}`", s, name, from),
                    None => format!("this takes ownership, and `{}` is still in `{}`", name, from),
                };
                Diagnostic::error(format!(
                    "cannot give away `{}` — `with` only lends it out of `{}`",
                    name, from
                ))
                .with_code("E0919")
                .with_primary(self.span, label)
                .with_secondary(*with_at, format!("`{}` is reached through this, not taken out", name))
                .with_fix(format!("change `{}` in place inside the block; to give the value away, take it out of `{}` first", name, from))
                .with_why(format!(
                    "`{}` still holds the value after the block, so giving `{}` away would leave two owners of one value. [mem.borrowing/W8]",
                    from, name
                ))
            }

            ConsumeBorrowedCapture { name, closure_at, ty, linear } => {
                let t = ty.as_deref().unwrap_or("…");
                let copy = if *linear {
                    String::new()
                } else {
                    format!("\n\nor give away a copy instead: `{}.clone()`", name)
                };
                Diagnostic::error(format!(
                    "cannot consume `{}` — a closure can't give away what it captured",
                    name
                ))
                .with_code("E0891")
                .with_primary(self.span, format!("this consumes `{}`", name))
                .with_secondary(*closure_at, format!("this closure captured `{}`", name))
                .with_fix(format!(
                    "pass `{n}` in instead of capturing it — a `take` parameter:\n\n  \
                     |take {n}: {t}| {{ … }}\n\n\
                     or, if this runs once as a task, a task block, which may consume what \
                     it captures:\n\n  \
                     spawn {{ … }}{copy}",
                    n = name
                ))
                .with_why(
                    "nothing says how many times a closure runs, so one `close()` in the \
                     body can be any number of closes at runtime. A `take` parameter is a \
                     new value on every call, so consuming it is fine [mem.closures/CM4]",
                )
            }

            BorrowedCaptureEscapes { path, root, ty, closure_at } => {
                Diagnostic::error(format!(
                    "`{}` is a capture — a closure can't return what it captured",
                    path
                ))
                .with_code("E0907")
                .with_primary(self.span, format!("`{}` isn't Copy, so this hands out `{}` itself", ty, path))
                .with_secondary(*closure_at, format!("this closure captured `{}`", root))
                .with_fix(format!("return a copy: `{}.clone()`", path))
                .with_why(
                    "a closure can't give away what it captured: nothing says how many \
                     times it runs, so every call would hand out the same value again, \
                     and the scope or environment that holds it still owns it \
                     [mem.closures/CM4]",
                )
            }

            EnsureMaybeConsumed { name, ensure_at, consumed_at } => {
                Diagnostic::error(format!(
                    "consumption of `{}` depends on which path ran",
                    name
                ))
                .with_code("E0821")
                .with_primary(*ensure_at, "cleanup scheduled here")
                .with_secondary(*consumed_at, format!("`{}` consumed only on this branch", name))
                .with_help(format!(
                    "exit inside the consuming branch (`return` right after consuming `{}`), \
                     or consume `{}` on every path",
                    name, name
                ))
                .with_fix(format!("move the exit inside the branch that consumes `{}`", name))
                .with_why("which cleanups run is decided at compile time (ctrl.ensure/C3) — a value \
                           consumed on some paths but not others has no definite answer at scope exit (C4)")
            }

            WithBlockStructuralMutation { collection, operation, via, binding_span } => {
                let (headline, primary) = match via {
                    Some(callee) => (
                        format!("`{}` can resize `{}`, and a `with` block is holding an element of it", callee, collection),
                        format!("`{}` can grow or shrink `{}` through this argument", callee, collection),
                    ),
                    None => (
                        format!("cannot {} `{}` inside `with` block", operation, collection),
                        format!("{} not allowed inside with block", operation),
                    ),
                };
                Diagnostic::error(headline)
                .with_code("E0808")
                .with_primary(self.span, primary)
                .with_secondary(*binding_span, "element borrowed here")
                .with_help("move the structural mutation outside the with block")
                .with_fix("move the structural mutation outside the with block")
                .with_why(format!(
                    "{} can reallocate, invalidating the borrowed element. If the elements \
                     reference each other and get deleted individually, that is what a `Rack` \
                     is for — a node never moves, so nothing invalidates [mem.borrowing/W2]",
                    collection
                ))
            }

            UseAfterDiscard { name, discarded_at } => {
                Diagnostic::error(format!("use of discarded value: `{}`", name))
                    .with_code("E0811")
                    .with_primary(self.span, "value used here after discard")
                    .with_secondary(*discarded_at, "value discarded here")
                    .with_help("remove the `discard` or restructure so the value isn't needed after this point")
                    .with_why("`discard` explicitly drops a value and invalidates its binding — using it afterwards is an error")
            }

            DiscardResource { name } => {
                Diagnostic::error(format!(
                    "cannot discard resource `{}` — use its consuming method",
                    name
                ))
                .with_code("E0812")
                .with_primary(self.span, "resource types cannot be discarded")
                .with_help(format!("call `.close()` or another consuming method on `{}`", name))
                .with_fix(format!("replace `discard {}` with `{}.close()`", name, name))
                .with_why("resource types must be consumed properly — `discard` would silently leak the resource")
            }

            ScopeLimitedClosureEscapes { name } => {
                Diagnostic::error(format!(
                    "closure `{}` captures scoped borrow and cannot escape",
                    name
                ))
                .with_code("E0862")
                .with_primary(self.span, "closure would outlive its captured borrow")
                .with_fix("give the closure a value it can keep — clone what it reads, or hold the source somewhere that outlives it")
                .with_why("a closure that outlives its frame carries what it captured (CM1), but a borrow lent by a `with` block or a borrowed parameter isn't the frame's to give, so there is nothing to carry (SL2)")
            }

            TaskWriteLost { name, spawn_span } => {
                Diagnostic::error(format!(
                    "`{}` is written in a task and nothing reads it back",
                    name
                ))
                .with_code("E0896")
                .with_primary(self.span, format!("this write lands on the task's own copy of `{}`", name))
                .with_secondary(*spawn_span, "the task gets a copy of every capture, and it dies with the task")
                .with_fix(format!(
                    "share the value instead: `let shared = Shared.new(…)`, capture `shared.clone()`, and write through `with shared.write() as {} {{ … }}`",
                    name
                ))
                .with_help("or return the value from the closure and read it off `join()`")
                .with_why("a task works on its own copy of what it captured, and `join()` is not a write-back — the write would be thrown away when the task ends")
            }

            HeldNodeDeleted { collection, link, holder_span, in_loop } => {
                let holder = if *in_loop { "the `for mutate` loop" } else { "the `with` block" };
                Diagnostic::error(format!(
                    "this can delete the node holding `{}`, and {} is still using it",
                    collection, holder
                ))
                .with_code("E0928")
                .with_primary(self.span, format!("the node `{}` names can be freed here", link))
                .with_secondary(*holder_span, format!("`{}` is held from here until the block ends", collection))
                .with_fix(format!(
                    "delete after the block ends, or copy what you need out of `{}` first",
                    collection
                ))
                .with_why(
                    "a node's fields live in the node, and deleting it frees them. Nodes never \
                     move, so deleting a different node is fine, but this one is where the held \
                     element lives [mem.racks/RK1, mem.borrowing/W2]",
                )
            }

            PointerDangles { name, pointee, loss, lost_at } => {
                use crate::PointerLoss;
                let (what, at, fix) = match loss {
                    PointerLoss::Resized { operation, via } => (
                        "changed size, so its storage may have moved".to_string(),
                        match via {
                            Some(callee) => format!("`{}` can resize `{}` here", callee, pointee),
                            None => format!("`{}` here can move `{}`'s storage", operation, pointee),
                        },
                        format!("take the pointer again after the change: `{} = unsafe {}.as_ptr()`", name, pointee),
                    ),
                    PointerLoss::Moved => (
                        "was moved, and its storage went with it".to_string(),
                        format!("`{}` moves here", pointee),
                        format!("use `{}` before `{}` moves, or take a pointer into wherever it went", name, pointee),
                    ),
                    PointerLoss::Replaced => (
                        "was replaced, which freed what it held".to_string(),
                        format!("`{}` is replaced here", pointee),
                        format!("take the pointer again after the assignment: `{} = unsafe {}.as_ptr()`", name, pointee),
                    ),
                    PointerLoss::OutOfScope => (
                        "went out of scope".to_string(),
                        format!("`{}` is freed at the end of this block", pointee),
                        format!("declare `{}` where `{}` is, so it lives as long as the pointer is used", pointee, name),
                    ),
                };
                Diagnostic::error(format!("`{}` points into `{}`, which {}", name, pointee, what))
                    .with_code("E0929")
                    .with_primary(self.span, format!("`{}` is used here", name))
                    .with_secondary(*lost_at, at)
                    .with_fix(fix)
                    .with_why(
                        "a raw pointer doesn't keep what it points into alive or in place. The compiler \
                         knows where each pointer came from, so a read here is an error rather than a \
                         read of freed memory [mem.unsafe/PT3]",
                    )
            }

            PointerEscapes { pointee, into } => {
                use crate::PointerSink;
                let (headline, primary, fix) = match into {
                    PointerSink::Return => (
                        format!("this returns a pointer into `{}`, which is freed when the function returns", pointee),
                        format!("points into `{}`", pointee),
                        format!("return `{}` itself, or allocate with `alloc` and have the caller free it", pointee),
                    ),
                    PointerSink::Place(place) => (
                        format!("`{}` outlives `{}`, so it can't hold a pointer into it", place, pointee),
                        format!("points into `{}`, which is freed when the function returns", pointee),
                        format!("store `{}` itself, or allocate with `alloc` and free it explicitly", pointee),
                    ),
                    PointerSink::Task(capture) => (
                        format!("`{}` points into `{}` and can't cross into a task", capture, pointee),
                        format!("the task may still run after `{}` is gone", pointee),
                        format!("move `{}` into the task instead, or share it through a `Shared`", pointee),
                    ),
                };
                Diagnostic::error(headline)
                    .with_code("E0930")
                    .with_primary(self.span, primary)
                    .with_fix(fix)
                    .with_why(
                        "a raw pointer doesn't keep what it points into alive. One into this frame's \
                         own values is good only while the frame is [mem.unsafe/PT2]",
                    )
            }

            PointerWriteUnderView { pointee, holder_span, in_loop } => {
                let holder = if *in_loop { "the `for mutate` loop" } else { "the `with` block" };
                Diagnostic::error(format!(
                    "this writes through a pointer into `{}` while {} holds a place in it",
                    pointee, holder
                ))
                .with_code("E0931")
                .with_primary(self.span, format!("writes into `{}`", pointee))
                .with_secondary(*holder_span, format!("`{}` is held from here until the block ends", pointee))
                .with_fix("write through the binding instead, or after the block ends".to_string())
                .with_why(
                    "while a binding holds a place, it's the one name that may change it. A write \
                     through a pointer would land under it and be lost or overwrite it [mem.unsafe/PT4]",
                )
            }

            ForMutateStructuralMutation { collection, operation, via, loop_span } => {
                let (headline, primary) = match via {
                    Some(callee) => (
                        format!("`{}` can resize `{}`, which a `for mutate` is iterating", callee, collection),
                        format!("`{}` can grow or shrink `{}` through this argument", callee, collection),
                    ),
                    None => (
                        format!("cannot {} `{}` during `for mutate` — invalidates iteration", operation, collection),
                        format!("{} not allowed during mutable iteration", operation),
                    ),
                };
                Diagnostic::error(headline)
                .with_code("E0814")
                .with_primary(self.span, primary)
                .with_secondary(*loop_span, format!("`{}` is being iterated here", collection))
                .with_help("collect changes and apply them after the loop")
                .with_why("structural mutations (insert, remove, push, clear) invalidate the iterator — elements may shift or be reallocated")
            }

            ForMutateTakeItem { item, collection, loop_span } => {
                Diagnostic::error(format!(
                    "cannot pass `{}` to `take` parameter — borrowed from `{}`",
                    item, collection
                ))
                .with_code("E0815")
                .with_primary(self.span, "would move element out of collection")
                .with_secondary(*loop_span, format!("`{}` is borrowed from `{}` during iteration", item, collection))
                .with_help(format!("clone `{}` before passing, or restructure to avoid taking ownership", item))
                .with_why("for-mutate borrows elements in place — taking ownership would leave a hole in the collection")
            }

            LinearWildcardDiscard { position, type_name } => {
                use crate::LinearDiscardPosition;
                let (label, help) = match position {
                    LinearDiscardPosition::Scrutinee => (
                        format!("`_` would silently drop linear value of type `{}`", type_name),
                        "name the value with `name` (or `Type as name`) and consume it on every arm".to_string(),
                    ),
                    LinearDiscardPosition::Field { constructor, field, index } => {
                        let where_ = match (field, index) {
                            (Some(f), _) => format!("field `{}` of `{}`", f, constructor),
                            (None, Some(i)) => format!("field {} of `{}`", i, constructor),
                            (None, None) => format!("a field of `{}`", constructor),
                        };
                        (
                            format!("`_` discards linear `{}` payload in {}", type_name, where_),
                            "bind the linear field by name and consume it (e.g. `try value.close()`)".to_string(),
                        )
                    }
                };
                Diagnostic::error(format!(
                    "wildcard would silently drop linear value of type `{}`",
                    type_name
                ))
                .with_code("E0816")
                .with_primary(self.span, label)
                .with_help(help)
                .with_why("linear values must be consumed exactly once — a `_` here would leak the resource [ER42/ER43]")
            }
        }
    }
}


/// `` `ensure conn.close()` ``, or `` `ensure tx.commit()` or `ensure tx.rollback()` ``
/// when the type has several consuming methods and nothing says which one
/// cleans up.
fn ensure_call(name: &str, consume: &[String]) -> String {
    if consume.is_empty() {
        return format!("`ensure {}.<consuming method>()`", name);
    }
    either(consume, "ensure ")
}

/// `` `a` ``, `` `a` or `b` ``, `` `a`, `b` or `c` `` — each call quoted and prefixed.
fn either(calls: &[String], prefix: &str) -> String {
    let quoted: Vec<String> = calls.iter().map(|c| format!("`{}{}`", prefix, c)).collect();
    match quoted.split_last() {
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} or {}", rest.join(", "), last),
        None => String::new(),
    }
}

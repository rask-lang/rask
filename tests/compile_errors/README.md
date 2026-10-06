# Compile Error Examples

This directory contains code that **should not compile**. Each file demonstrates specific safety guarantees enforced by the Rask compiler.

Each `// ERROR:` comment claims the compiler rejects the code below it. If the
compiler accepts it, that's a compiler bug — the spec says it should be rejected.

**Markers are anchored to a line.** The check is: between one marker and the
next, at least one diagnostic must point at a line in that span. So put the
marker directly above (or on the end of) the line that should be rejected. A run
of consecutive `// ERROR` lines counts as one marker — several lines often
describe a single rejection.

Markers no diagnostic answers yet are listed in
[DEAD_MARKERS.txt](DEAD_MARKERS.txt) with a per-file count. That count may only
go down.

## Files

### Syntax

| File | What it tests |
|------|--------------|
| [syntax_rejected.rk](syntax_rejected.rk) | Rust-isms (`pub`, `fn`, `::`, `let mut`, turbofish, `&`), `const` in a body. Parser errors only — a rule the checker enforces can never fire here, so those markers moved to files of their own |
| [rust_syntax_rejected.rk](rust_syntax_rejected.rk) | Additional Rust keyword rejections |
| [rust_error_propagation.rk](rust_error_propagation.rk) | Rust's `?` used to propagate an error (ER12, E0368) — Rask spells that `try`, and `?` is the presence test, which a `T or E` can't answer. The marker lived in `syntax_rejected.rk` below eight parse errors that stop the pipeline before the checker runs |
| [interface_body_members.rk](interface_body_members.rk) | Anything an interface body doesn't hold — `const`, a nested `struct`, a bare `public`, an attribute (#1164). All of them used to hang the parser: the body loop had no branch for them, so nothing consumed the token and the condition never went false. `type` was on this list and no longer is — an interface body holds methods and associated types (TD4) |
| [interface_shadows_stdlib.rk](interface_shadows_stdlib.rk) | A program's `interface Writer` is its own interface, not the stdlib's of the same name (#1329). `Buffer` implements the stdlib's, so it fails a bound on the program's even with matching methods. The name lookup used to merge them, which also broke the stdlib's own conformances |
| [interface_generic_and_assoc.rk](interface_generic_and_assoc.rk) | The ways a generic interface or an associated type goes wrong (#1164, #1165): a header that leaves the parameter unbound, a method whose signature isn't the one the applied interface asks for, a conformance that never says what `Out` is, a binding naming a member the interface doesn't declare, `Self.X` for an undeclared `X`, and an `Out` that fails the bound the interface put on it. Each used to surface as "this type is missing methods the interface requires" pointing at a block that had them. Also `type.operator-resolution/OR1`: an operator whose operand pair names no conformance, where the left operand has the method but no header |
| [extend_header_bound.rk](extend_header_bound.rk) | A bound written in an `extend Holder<T: Named>` header (`type.generics/GF6`). The bound is the struct's and every method already has it, so the header doesn't restate it; this used to be a bare "Expected '>'" (#1364) |

### Type System

| File | What it tests |
|------|--------------|
| [type_errors.rk](type_errors.rk) | Implicit bool conversion, narrowing `as`, float comparison, Option no-auto-unwrap, try type mismatch, branch type mismatch, break value types |
| [cast_rules.rk](cast_rules.rk) | `as` cast rules: narrowing (CV2), sign reinterpret (CV3), float→int (CV4), int→char (CH5), int↔bool (BL3); conversion methods used where their policy means nothing — `floor` on an integer, `wrap` to a float, `round` int→int, `clamp` on a float (CV11–CV16, E0818); `as` to a collection or a struct, which reinterprets bits rather than converting and needs `unsafe` to say so (E0838, #862) |
| [unknown_lowercase_type.rk](unknown_lowercase_type.rk) | An unknown type name is unknown whatever its case (#966) — `str` (not a Rask type; the string type is `string`), a typo'd `uszie`, an invented `zqzq`. The check used to require a leading capital, so these were accepted at the signature and only failed at the use site. Asserts the real lowercase primitives aren't swept up |
| [comptime_field_name.rk](comptime_field_name.rk) | `value.("x")` naming a field that doesn't exist (E0312, #930). Field lowering answers "field 0" for a name it can't find, so an unchecked literal read the first field and reported nothing. The comptime-known half of the rule lands at MIR instead — a `comptime for` binding's name isn't knowable until the loop unrolls |
| [index_types.rk](index_types.rk) | Index expression types: integer for Vec/array/string, `K` for Map (#310, V1) |
| [list_literal_deferred_slot.rk](list_literal_deferred_slot.rk) | A `[...]` argument to a method on a receiver that isn't known yet (`Sum.new().last_of([4, 5])`) waits for the call before taking its shape (#1457). It still has to fit: a wrong array length or a non-collection parameter is a mismatch at the literal |
| [sort_needs_comparable.rk](sort_needs_comparable.rk) | `sort`, `min`, `max` and `sort_by_key`'s key need `Comparable` (SO3), so a `Vec<i64?>` can't be sorted (#1313). Also a `Vec` method from a `where`-bound block called on elements that miss the bound (`Vec<f64>.hash()`) |
| [no_slice_type.rk](no_slice_type.rk) | `[]T` written as a type. There is no slice type — a run of elements is a `Vec<T>`, part of one is `v.skip(a).take(n)`. Parse errors only; the checker's half is in range_slice_rejected.rk |
| [range_slice_rejected.rk](range_slice_rejected.rk) | `v[a..b]` on a Vec or a fixed array (E0819, IX4). Only a string slices to another string; a Vec copy behind a bracket would hide the allocation |
| [type_test_on_plain_value.rk](type_test_on_plain_value.rk) | `p is Point` on a `Point` (E0398, ER23). A plain value has one type, so the test is decided by the source. The bare form used to be read as a binding, true on the interpreter and false natively (#1352) |
| [rack_node_not_struct.rk](rack_node_not_struct.rk) | A `Rack<T>` whose node isn't a struct (E0326, RK14, #1297): annotated, inferred from the insert, and an enum. Native used to accept `Rack<i32>` and the interpreter failed at the first insert. A generic `Rack<T>` passes |
| [rack_link_use_after_delete.rk](rack_link_use_after_delete.rk) | Using a `Link<T>` after `rack.delete` freed its node (E0328) — reported as a use after free, not a move — a read, a write, and `contains`. The locals rule: a non-optional link asserts its node is alive, so a delete makes the use contradict the type |
| [non_optional_link.rk](non_optional_link.rk) | A required edge (`Link<T>`, no `?`) is unsupported for now (E0327) — it needs a batch to construct and a cascade/restrict policy to destroy, neither built. Bare links inside `Vec`/`Map` stay legal |
| [link_not_orderable.rk](link_not_orderable.rk) | `<`, `<=`, `>`, `>=`, `compare`, and `sort`/`min`/`max` over a `Vec<Link<T>>` (E0406, RK11, #1266) — all of them ordered by address, which moves with whatever the program allocated before the rack. `==`, `!=` and ordering by a node's own field stay legal |
| [take_self_through_link.rk](take_self_through_link.rk) | A `take self` method called through a `Link<T>` (E0910, RK1/RK2). A link reads like its node, so `self` and `mutate self` methods borrow it in place (#1285); consuming the receiver would move the node out of the rack that owns it |
| [not_iterable.rk](not_iterable.rk) | `for` over something with no elements — an integer, a string, a struct (E0827), and an index type check that only reaches a container arrived at through a field (#632) |
| [implicit_widening_limits.rk](implicit_widening_limits.rk) | The int→int pairs CV1a does *not* make implicit: `u64`→`i64`, `i64`→`u64`, `u32`→`i32`, `u8`→`i8`, and plain narrowing (CV1a, CV2) |
| [int_float_arithmetic.rk](int_float_arithmetic.rk) | `+ - * /` between an integer and a float variable (CV1a, E0401, #816) — native used to drop the float operand and answer with an integer. An unsuffixed literal still takes the float slot |
| [mixed_signedness_arithmetic.rk](mixed_signedness_arithmetic.rk) | `+ - * / %` and `& \| ^ << >>` between a signed and an unsigned integer (ORD4, E0371) — comparison is the exception and stays legal (#778) |
| [int_literal_range.rk](int_literal_range.rk) | An integer literal past the slot it lands in: needs 128 bits in an `i64`, one past `i128::MAX`, negative in a `u128` (E0825, #800) |
| [int_literal_unwritable.rk](int_literal_unwritable.rk) | The two ends no type holds — digits past `u128::MAX` (lexer) and a negative below `i128::MIN` (parser sign fold) (#800) |
| [untyped_bindings.rk](untyped_bindings.rk) | Bindings that carried no type at all, so a wrong annotation unified happily: a struct-variant pattern's fields, an `is` binding, a tuple `for` binding (E0308, #809) |
| [newline_continuation.rk](newline_continuation.rk) | A line starting with `+` — excluded from newline continuation (P3) and not a statement either (#304) |
| [bare_shared_with.rk](bare_shared_with.rk) | Bare `with shared as v` — the lock has to be named `.read()` or `.write()` (conc.sync/R4, E0839, #880). Nothing enforced it: the interpreter hit a self-contradictory runtime error and native read the wrong bytes |
| [spawn_needs_multitasking.rk](spawn_needs_multitasking.rk) | `spawn` with no `using Multitasking { }` scope (conc.async/CC1 and CC2, std.testing/T17). CC1 was keyed off the qualified `async.spawn` spelling, so a bare `spawn(|| { … })` was never checked; CC2 worked but never walked `test` blocks. The error lands where the block belongs — at the `spawn` only in a root nothing calls (entry point, `test` block, `@test` function), at the call site otherwise, so a library function may spawn and leave the scope to its caller |
| [local_shared_sent.rk](local_shared_sent.rk) | A `Shared.new` box — the `Local` strategy, no lock — captured by a spawned task (`conc.sync/SH7`, E0346). Both locking strategies in the same file must still compile |
| [interface_static_call.rk](interface_static_call.rk) | A method called on an interface's name, `Labeled.label(d)` (E0897, MN1) — it used to type-check as a static call and die in MIR lowering |
| [method_name_clash.rk](method_name_clash.rk) | Two blocks each defining `label` on one type (E0898, MN2) — two `extend` blocks, and two conformances to different interfaces; the last block read used to win silently |
| [method_outside_interface.rk](method_outside_interface.rk) | A plain method inside an `implements` block (E0893, CD2) — the block is the contract, so the helper is sent to `extend T { }` |
| [map_key_hashable.rk](map_key_hashable.rk) | A Map key that isn't Hashable (E0834, HA1/HA4, #812) — a nominal newtype with no `implements …` clause, a float, a struct with a float field; each gets the way out that fits it |
| [generic_arg_identity.rk](generic_arg_identity.rk) | A user type as a generic argument keeps its identity — a wrong Map key or value on `Map<K, V>.new()` (E0392/E0308, #812) |
| [type_mismatch_arg.rk](type_mismatch_arg.rk) | Wrong argument type |
| [type_mismatch_return.rk](type_mismatch_return.rk) | Wrong return type |
| [wrong_arg_count.rk](wrong_arg_count.rk) | Wrong number of arguments |
| [named_args_out_of_order.rk](named_args_out_of_order.rk) | A named argument whose label isn't the parameter in its position (E0903, #1347) — swapped on a free function, a method, a static method, a struct variant, a stdlib method and a defaulted call; a label naming no parameter; a label on a tuple variant or a closure value, which have no names. Labels were never read, so a swapped call bound by position |
| [error_mismatch.rk](error_mismatch.rk) | Incompatible error types with `try` |
| [try_shape_rule.rk](try_shape_rule.rk) | Bare `try` whose other branch doesn't fit the return (ER47, E0399/E0400) — an absence in a `T or E` function, an error in a `T?` function (#598) |
| [error_interface_variants.rk](error_interface_variants.rk) | Picking a variant off `Error` (E0863, #1095) — it's the interface every error implements, not an enum, so `Error.NotFound` names nothing. Both a plausible spelling and an invented one, since neither used to be caught |
| [ambiguous_error_wrap.rk](ambiguous_error_wrap.rk) | Two variants of the error enum wrap the same error (ER31a, E0359) — `try` asks which instead of picking |
| [optional_operators_need_optionals.rk](optional_operators_need_optionals.rk) | `??`, `!` and `take` on something that can never be absent (OPT3/OPT11/OPT13/OPT32, E0831/E0832/E0365) — including `m[k] ?? d`, which points at `.get(k)`, and an operand whose type settles only at the end of checking (#1290) |
| [interface_bound_messages.rk](interface_bound_messages.rk) | What a failed interface requirement says, per source: a numeric bound (E0333, members not methods), an ordinary generic bound, a conformance header, an `as any Interface` cast, and a bound naming an interface nobody declared (E0833, did-you-mean) |
| [bound_names_its_param.rk](bound_names_its_param.rk) | A bound naming a type parameter, `T: Mul<T>` or `T: Mul<K>`, is checked with the call's arguments filled in (E0333, #1463). It used to ask for the literal `Mul<T>`, so a type that did conform was rejected; this file keeps the ones that don't |
| [struct_bound_at_construction.rk](struct_bound_at_construction.rk) | A struct or enum's bound on its parameter is checked wherever the type is instantiated: a literal, a variant constructor, a written annotation (E0333, `type.generics/GF6`, #1462). Only generic calls used to check one, so `Holder { item: 5 }` with `T: Named` was accepted |
| [no_auto_wrap_outside_return.rk](no_auto_wrap_outside_return.rk) | A bare `T` becomes a `T or E` at `return` only (ER11, E0828) — binding, argument (free *and* method), and field are rejected, and the optional shape is exempt |
| [error_type_named_in_diagnostics.rk](error_type_named_in_diagnostics.rk) | Three codes that mention a `T or E` all name its error type rather than leaking `<type#N>` (#646) |
| [unknown_type_name.rk](unknown_type_name.rk) | Typo'd type name in signature (PC2) — errors instead of becoming a generic |
| [interface_signature_unknown_type.rk](interface_signature_unknown_type.rk) | An unknown type name in an interface method's signature (PC2, E0356, #1164). Every other signature position had this check — free function, method, struct field — the interface was the gap, so `register_interface` parsed the types and never read their names. A single uppercase letter stays a type parameter (PC1) |
| [type_called_as_function.rk](type_called_as_function.rk) | A struct or enum name in call position (E0345) — `Name(value)` is the nominal-type constructor (T7), structs have no tuple form (S1) |
| [single_letter_type_name.rk](single_letter_type_name.rk) | Single-letter concrete type names are reserved for type parameters (PC3) |
| [not_displayable.rk](not_displayable.rk) | Rendering a type that can't render itself: a struct that never opted in, an optional with no missing case (D3, D4) — through `{}` and through `print`/`println` as a call (#772) |
| [unimplemented_module_fn.rk](unimplemented_module_fn.rk) | A stdlib module function marked `@unimplemented` — caught at the call instead of segfaulting there (#506) |
| [nominal_interface_not_listed.rk](nominal_interface_not_listed.rk) | A nominal newtype inherits only the interfaces its `implements …` clause lists (T10) |
| [duplicate_conformance.rk](duplicate_conformance.rk) | Two `T implements Interface` blocks claiming the same pair (E0407, XC3). Conformances were filed in a set, so the second landed on the first while its methods still registered, and the last block read supplied them. Reordering two files changed what the program did, silently. Also covers a program block landing on a *stdlib* one, which is an override and not a clash (XC2) — the program's block takes the slot, so a second program block is blamed on the program's first, never on the stdlib's. Also pins the no-double-report property: two *different* applied forms of one generic interface are two conformances, so that collision is MN3's E0889 alone. Asserts the legal shapes below it stay legal: two interfaces on one type, and one block naming both |
| [no_encode_opt_out.rk](no_encode_opt_out.rk) | Serializing a type marked `@no_encode` / `@no_decode` (E0408, E16). The annotation parsed and did nothing — nothing in the compiler knew the name, so `json.encode` on a `@no_encode` struct printed the fields. Covers both directions, both through `json` and through a bound the program declares itself, and asserts a field with no wire representation stays E0388: this error names the annotation because there is no offending field to point at |
| [missing_return.rk](missing_return.rk) | Function without return statement |
| [interface_bound_unsatisfied.rk](interface_bound_unsatisfied.rk) | Type argument doesn't implement the bound's interface (#314) |
| [interface_bound_missing_method.rk](interface_bound_missing_method.rk) | Method not provided by the type param's bounds (#314) |
| [generic_disjointness.rk](generic_disjointness.rk) | Generic instantiation collapses `T or E` into `E or E` (ER3a, #488) — free function, propagated through a generic caller, and a method on a generic receiver |
| [catch_void_body_blame.rk](catch_void_body_blame.rk) | A void-bodied `catch` on a value whose success type is still open (ER14a, #876) — the checker used to decide that type from whichever catch it saw first instead of checking against it, so the void body's own mismatch went unreported and a later, correctly-typed use of the same value was blamed instead |

### Ownership & Borrowing

| File | What it tests |
|------|--------------|
| [ownership_errors.rk](ownership_errors.rk) | Use-after-move, conditional move, @unique, @resource leak/double-consume, Vec never Copy |
| [linear_containers.rk](linear_containers.rk) | Vec/Map can't hold linear elements (RC1/RC3): annotation, push, param, return, field, transitive, nested, optional, alias, Map value/key (E0820) |
| [linear_generic_instance.rk](linear_generic_instance.rk) | A generic body re-checked for the linear type it is called with (#1366, E0904): a `T` dropped in the body, a payload matched out of `List<Conn>` and dropped, a drop two generic calls down blamed on the outer call, a generic method and a method of a generic type (`Holder<T>.drop_it(take self)`) that drop theirs, a `take self` on a concrete holder that leaks its resource field, and a `T` handed to two `take` parameters |
| [branch_merge.rk](branch_merge.rk) | Branch-merge soundness (O3, L1): move/consume on one branch of if, if-without-else, and match arms; move inside a loop body |
| [borrow_errors.rk](borrow_errors.rk) | Mutating read-only param, moving from borrow, storing slices, borrow escape, structural mutation in `with`, non-Copy element binding |
| [borrow_stored.rk](borrow_stored.rk) | `string[..]` written as a type — a slice expression has no type spelling, because the storable form is `StringView`, which refcounts the source buffer rather than borrowing it. Parser-level only; the S3 rejection for keeping a slice past its statement is in borrow_errors.rk |
| [mutate_marker_required.rk](mutate_marker_required.rk) | An argument to a `mutate` parameter with no `mutate` marker (PM4/PM5, E0373) — a Copy argument and a field path are no exception; a method receiver is exempt; the marker on a non-`mutate` parameter is E0306 (#530) |
| [mutate_through_binding.rk](mutate_through_binding.rk) | Writing through a name a test or a pattern introduced (E0372, #788) — `if x? as v`, a `mutate` argument, a plain `for` element, a match-arm payload, `while x? as v`. `for mutate` and write-back through the original stay legal |
| [mutate_param_left_empty.rk](mutate_param_left_empty.rk) | A `mutate` parameter consumed and not replaced (PM2, E0836, #815) — outright and on one path only; consume-and-replace stays legal, and `take` is how a function says it keeps the value |
| [heap_not_consumed.rk](heap_not_consumed.rk) | A `Heap(…)` value that nothing consumes, one consumed twice, one handed to a `take` parameter and then dropped, and one consumed on only one branch (mem.linear/L1, L3, E0837, E0800, #819) |
| [consume_borrowed_param.rk](consume_borrowed_param.rk) | Giving away a parameter the caller only lent (PM1/L1, E0835, #804) — a `take self` method, a `take` parameter, and storing it into a field, which used to be reported as a borrow conflict about a mutation that wasn't happening (#818); `take` on the declaration is the way to say it |
| [field_view_stored.rk](field_view_stored.rk) | A non-Copy field read given a second owner (S1, S3, E0909): assigned into a field or variable, put in a struct literal, tuple, variant payload or `Heap`, passed to a `take` parameter or `push`, or bound with `let` first and stored or returned from there. A field read is a view, so the owner would share it with the source. The assignment was reported as a write to the source, or compiled from a local and freed the Vec twice (#1283); every other form compiled and native freed the caller's Vec (#1459). Building a new value out of the place being assigned stays legal |
| [moves_into_literals.rk](moves_into_literals.rk) | A value stored in a tuple or struct literal moves into it (O2, E0800); a borrowed parameter can't go into one (E0835); a loop's element can't be pushed or bound to a new name (LP6, E0902); `T: Copy` rejects a `Vec` at the call (G1a, E0333) |
| [to_vec_of_lent_items.rk](to_vec_of_lent_items.rk) | `to_vec` over a chain of only lending adapters (`filter`, `take`, `skip`, `enumerate`) whose item isn't Copy (SEQ47, E0905). The chain owns nothing to move in and `to_vec` doesn't deep-clone; this compiled, and natively both vectors freed the same inner vectors (#1415) |
| [borrowed_match_part_given_away.rk](borrowed_match_part_given_away.rk) | A part matched out of a borrowed value given away (E0899). The arm's bindings are views into the caller's value: reading one is fine and no longer asks to be consumed, closing one is the error. The `take` version still owes each part |
| [borrowed_payload_returned.rk](borrowed_payload_returned.rk) | A payload matched out of a borrowed value, returned (E0872, #1425). `return self.items` was already rejected; `if self is Arr(a) { return a }` hands back the same storage and got through, which is how `JsonValue.as_array(self)` double-freed natively. `take self` or `.clone()` are the two ways out |
| [return_borrowed_param.rk](return_borrowed_param.rk) | A borrowed or `mutate` parameter returned whole (E0872, #1452), directly or as the value of an `if`/`match` branch, and the payload `o? as v` reads out of a borrowed optional. The field check skipped the whole value on the belief that another rule had it, and none did, so the caller got its own value back under a second name |
| [closure_param_borrowed.rk](closure_param_borrowed.rk) | The same two rules for a closure's parameters, which are borrows (CP1) with no `take` form (CP4): returned whole from a block or an expression body (E0872), or given to a `take` parameter (E0835). The ownership pass registered them as owned, so `|p: Vec<i64>| { return p }` aliased the caller's vector (#1458) |
| [with_guard_escapes.rk](with_guard_escapes.rk) | A `with` guard's bare identifier returned as the block's own value (#559, E0829) — struct payload rejected, field read/method call/scalar payload still compile |
| [small_size_fence.rk](small_size_fence.rk) | `@small` types over the 16-byte copy threshold (SM2, E0374) — a three-`i64` struct and a two-`string` one; plus the generic half, where `Pair<i64>` fits and `Pair<string>` doesn't (SM3, E0375) (#587) |
| [ensure_cancellation.rk](ensure_cancellation.rk) | `ensure` cancellation must be statically definite (C3/C4): resource consumed on some merging paths but not all — if-without-else, single match arm, nested block (E0821) |
| [ensure_consumes_nothing.rk](ensure_consumes_nothing.rk) | An `ensure` whose body only reads the resource — a borrowing method, a field read (L4, E0908). What it commits is what its body consumes, found by the same walk as any other consume. The receiver slot used to count whatever was called on it, so `ensure c.peek()` leaked silently while `ensure log.record(c.release())` was rejected (#1301) |

### Pattern Matching

| File | What it tests |
|------|--------------|
| [match_errors.rk](match_errors.rk) | Non-exhaustive match, wildcard on linear resource, guard without diverge, or-pattern binding mismatch |
| [nonexhaustive_match.rk](nonexhaustive_match.rk) | Non-exhaustive enum match |
| [guarded_match_not_exhaustive.rk](guarded_match_not_exhaustive.rk) | A guarded arm covers nothing (#1402): a match whose only catch-all, or only arm for a variant, is guarded is non-exhaustive (E0864, E0340). It used to check and stop at runtime |

### Closures

| File | What it tests |
|------|--------------|
| [closure_errors.rk](closure_errors.rk) | What the parser rejects around closures: `\|mutate x\|` capture syntax (unimplemented, #1087 — the message used to suggest `\|mutate x: T\|`, which compiles and means something else) and a closure type in a signature. MC2 and SL2 can't be reached until those exist |
| [closure_returns_borrowed_capture.rk](closure_returns_borrowed_capture.rk) | A closure that stays in its frame giving away a non-Copy capture (E0907, E0891, #1449): returning it (`\|\| b`), a field of it, or handing it to a `take`. It points at the variable, so the caller got the frame's own value on every call. `.clone()`, a Copy part, and a shadowing local stay legal |
| [task_lost_write.rk](task_lost_write.rk) | A task writing a capture nothing reads back (SP1, E0896, #1281) — inline, under an `if`, through a closure named before it was spawned, a read that comes before the write, and an accumulator loop whose every write is read by the next iteration. The legal shapes sit below: a task that returns what it summed, and one that counts for its own output |

### Other

| File | What it tests |
|------|--------------|
| [tag_shape.rk](tag_shape.rk) | The two malformed `@tag` shapes (E24a/E24b, E0841/E0842) — an unnamed payload with no key to tag, and a payload field that shadows the tag. Both compiled and panicked at runtime before: native wrote a duplicate JSON key, the interpreter dropped the tag (ctrl.panic/S7) |
| [field_annotation_forms.rk](field_annotation_forms.rk) | Serialization annotations the compiler can't act on (E19/E21, E0376) — the old `@skip` spelling, and `@rename` given a bare name or a number instead of a string literal; plus an excluded field with no default, which blocks auto-`Decode` (E13a, E0377) (#603) |
| [module_needs_import.rk](module_needs_import.rk) | A stdlib module used with no import for it (IM1, E0210) — `json` and `net` were exempt because `stdlib/http.rk` imports them into a scope shared with user code (#780) |
| [local_shadows_module.rk](local_shadows_module.rk) | A local named like a module the program imported (IM8, E0911): `let b = 5` under `import bits as b`, and `mut json` under `import json`. It was reported as "`b` is a built-in type"; the error now names the import and points at it (#1475) |
| [stdlib_renames.rk](stdlib_renames.rk) | task-2b rename sweep (#302): old stdlib names are hard errors, not aliases — `recv`/`try_recv`, `as_secs`/`as_secs_f64`, removed `File.lines()` (E0313), `os.getpid`/`os.vars`, `fs.read_file`/`write_file`/`append_file` (E0411) |
| [unknown_module_function.rk](unknown_module_function.rk) | A call through a module to a function it doesn't have (E0411, #1404). `async.join_all` used to be dropped unreported because `async` has no namespace struct; the message now names `Handles` as the type that has it |
| [private_stdlib_function.rk](private_stdlib_function.rk) | A call to a stdlib module function declared without `public` (E0412, #1410) — `json.parse`, the body behind `json.decode<JsonValue>`, and `time.wall_clock_nanos`. The marker wasn't checked, so the suite came to call `json.parse` |
| [method_visibility.rk](method_visibility.rk) | A method called from code that may not see it (E0413, #1417): a `private` method from `main` and from another type's `extend` block (V5), and `Range.last_value`, which the stdlib declares without `public` (V1, V2). A conformance's method (`JsonValue.to_string`) stays callable without `public` of its own |
| [let_reassign.rk](let_reassign.rk) | Reassigning a let binding |
| [shared_access_closure.rk](shared_access_closure.rk) | A closure handed to a blocking `read`/`write` on a `Shared` (E0900, #1311). Blocking access is a `with` block or one expression; closures are for `try_read`/`try_write`. The checker accepted the closure form, undeclared, and native read a slot nobody wrote |
| [link_sent_to_task.rk](link_sent_to_task.rk) | A link captured by `spawn` (E0901, #830) — on its own, optional, in a Vec, in a struct field, or as a channel's element. A link is its node's address, so two tasks would write one node unordered. A copied field and a whole rack still cross |
| [thread_handle_unjoined.rk](thread_handle_unjoined.rk) | A `Thread` handle dropped without a join or detach (E0805, #1360), and one put in a `Vec` (E0820). Several go in a `Handles` |
| [read_lock_mutate.rk](read_lock_mutate.rk) | Mutating through a `shared.read()` with-binding (E0360, conc.sync/R1) |
| [undefined_variable.rk](undefined_variable.rk) | Using undefined variable |
| [comptime_loop.rk](comptime_loop.rk) | Comptime iteration limits |
| [resource_leak.rk](resource_leak.rk) | Resource type not consumed |
| [result_match_by_variant.rk](result_match_by_variant.rk) | A `T or E` match covers `E` with an arm per variant; a fieldless variant arm is not a catch-all |
| [optional_resource.rk](optional_resource.rk) | A `@resource` inside an optional is still linear — the binding, the `? as` payload, and a `none` that gets filled (E0805, mem.linear/L1, #827) |
| [resource_field_debts.rk](resource_field_debts.rk) | A holder owes each resource field separately — closing one leaves the others, reported by field path (E0805, mem.linear/L1, #828) |
| [context_missing.rk](context_missing.rk) | Missing pool context clause |
| [context_ambiguous.rk](context_ambiguous.rk) | Ambiguous pool context |
| [context_unavailable.rk](context_unavailable.rk) | Pool context not in scope |
| [context_unnamed_structural.rk](context_unnamed_structural.rk) | Unnamed context used as binding |
| [link_escapes_in_collection.rk](link_escapes_in_collection.rk) | A `Vec<Link<T>>` returned out of the scope that owns its rack (E0379, #941) — the expression walk saw neither a link nor the push that put one in |
| [package_state_unsynchronized.rk](package_state_unsynchronized.rk) | Writing a bare `const Vec`/`const Map` (E0856, structure.modules/PS2, #944) — a data race out of safe code that lost updates and corrupted the heap |
| [inline_sync_unchained.rk](inline_sync_unchained.rk) | An inline `.read()` used as an operand rather than chained (E0339, conc.sync/R5, #958) — it printed 49 natively and 42 on the interpreter for a box holding 41 |
| [catch_binding_type.rk](catch_binding_type.rk) | The value `catch e =>` binds, checked against an annotation (E0308, #950) — it used to satisfy any type, which is how `grep_clone` lost every error message it printed |
| [enum_payload_mismatch.rk](enum_payload_mismatch.rk) | A variant payload that genuinely doesn't match, reported with the declared type as the expectation (E0308, #922) — it used to be named as "found" |
| [wide_scalar_copy_threshold.rk](wide_scalar_copy_threshold.rk) | An `i128` field counting its real 16 bytes, so a 24-byte struct moves instead of copying (E0800, #936) — and reports once, not twice (#1092) |
| [unknown_allow_name.rk](unknown_allow_name.rk) | `@allow(name)` where nothing answers to `name` (E0855, #1085) — the warning fired as if the annotation weren't there, which is exactly what a correctly-suppressed one looks like |
| [container_turbofish.rk](container_turbofish.rk) | `Vec.new<string>()` followed by a push of the wrong type (#1084) — the written type argument used to be dropped, so the binding stayed open and widened instead |
| [context_on_entry_point.rk](context_on_entry_point.rk) | A `using` clause on the entry point (CC11, E0831) — nothing can supply the hidden param, so it used to run on garbage (#732) |

## Running Tests

```bash
cd compiler
cargo test --release -p rask-cli --test compile_run every_compile_error_marker
```

That test walks this directory, so a new fixture is covered the moment it lands
— no registration step. Individual files also have their own tests in
`compiler/crates/rask-cli/tests/compile_run.rs` that assert the *wording* of the
diagnostic, which the marker gate doesn't look at.

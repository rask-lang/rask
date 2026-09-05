// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Type checker error types.

use rask_ast::Span;

use crate::types::{Type, TypeVarId};

/// Which way out of an unhashable Map key to offer. The type decides: a float
/// can never be a key, while a struct or a newtype just hasn't said it hashes
/// yet — and those two say it in different syntax.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapKeyFix {
    /// HA4: `f32`/`f64` are excluded outright. Key on the bits instead.
    Float,
    /// A nominal newtype — the traits it inherits are the ones its `with (…)`
    /// clause names.
    NominalClause,
    /// Anything else — an `extend T with Hashable` block declares it.
    ExtendBlock,
}

/// A type error.
#[derive(Debug, thiserror::Error)]
pub enum TypeError {
    #[error("type mismatch: expected {expected}, found {found}")]
    Mismatch {
        expected: Type,
        found: Type,
        span: Span,
    },
    #[error("undefined type: {0}")]
    Undefined(String),
    /// Inference finished and this binding's type is still open. Either nothing
    /// in scope pinned it down (the program needs an annotation) or inference
    /// has a gap (our bug) — the message says both, because the compiler can't
    /// tell which from here.
    #[error("couldn't work out the type of `{name}`")]
    UnresolvedType {
        name: String,
        /// A concrete type worth suggesting, when the shape is known enough to
        /// guess at one — `Vec<…>` for a `Vec` whose element is open.
        hint: Option<String>,
        span: Span,
    },
    #[error("arity mismatch: expected {expected} arguments, found {found}")]
    ArityMismatch {
        expected: usize,
        found: usize,
        span: Span,
    },
    #[error("type {ty} is not callable")]
    NotCallable { ty: Type, span: Span },
    #[error("no such field '{field}' on type {ty}")]
    NoSuchField { ty: Type, field: String, span: Span },
    /// An operator whose two sides can't be compared or combined.
    ///
    /// Mixed signedness is allowed on purpose (ORD4); `char` against an integer
    /// is the case this exists for, because native compares a `char` as its
    /// underlying scalar and quietly answers by code point.
    #[error("cannot apply `{op}` to {left} and {right}")]
    IncomparableOperands {
        left: Type,
        right: Type,
        /// The operator as written (`*`, `==`), not the desugared method name.
        /// `operator_spelling` in the resolver is the one table for this; the
        /// diagnostic had started keeping a second, shorter copy.
        op: String,
        span: Span,
    },
    #[error("no such method '{method}' on type {ty}")]
    NoSuchMethod {
        ty: Type,
        method: String,
        span: Span,
    },
    /// A stdlib method declared `@unimplemented` — the signature exists so the
    /// API can be referenced, but nothing implements it on either backend.
    #[error("`{ty}.{method}` is declared but not implemented yet")]
    UnimplementedStdlibMethod {
        ty: String,
        method: String,
        span: Span,
    },
    /// std.fmt/D4: `{}` (and a bare `to_string()`) needs `Displayable`, and
    /// structs opt in (D3). Optionals and results never render on their own.
    #[error("`{ty}` does not implement `Displayable`")]
    NotDisplayable {
        ty: String,
        /// `Some` when it came from an interpolation, so the message can name
        /// the placeholder instead of a `to_string()` the user never wrote.
        interpolated: bool,
        span: Span,
    },
    /// #314: method called on a type param whose bounds don't provide it.
    #[error("no method '{method}' provided by the bounds on `{param}`")]
    UnboundedTypeParamMethod {
        param: String,
        method: String,
        bounds: Vec<String>,
        span: Span,
    },
    #[error("infinite type: type variable would create infinite type")]
    InfiniteType { var: TypeVarId, ty: Type, span: Span },
    #[error("cannot infer type")]
    CannotInfer { span: Span },
    #[error("invalid type string: {0}")]
    InvalidTypeString(String),
    /// type.gradual/PC2: PascalCase name in a signature resolves to nothing
    #[error("unknown type `{name}`")]
    UnknownTypeName {
        name: String,
        /// Closest declared type name, if any
        suggestion: Option<String>,
        span: Span,
    },
    /// type.gradual/PC3: single uppercase letters are reserved for type parameters
    #[error("single-letter type name `{name}` is reserved for type parameters")]
    SingleLetterTypeName {
        name: String,
        kind: String,
        span: Span,
    },
    #[error("try can only be used in functions returning Option or Result, found {return_ty}")]
    TryInNonPropagatingContext { return_ty: Type, span: Span },
    #[error("error type mismatch in `try`: propagating `{inner_err}`, but function returns `_ or {outer_err}`")]
    TryErrorMismatch { inner_err: String, outer_err: String, span: Span },
    /// ER31a: more than one variant of the boundary enum wraps the same error
    /// type, so `try` can't pick one on its own.
    #[error("`try` can't tell which variant of `{outer_err}` should wrap `{inner_err}`")]
    AmbiguousErrorWrap {
        inner_err: String,
        outer_err: String,
        variants: Vec<String>,
        span: Span,
    },
    #[error("try can only be used within a function")]
    TryOutsideFunction { span: Span },
    /// ER47: bare `try` on an optional inside a function that returns `T or E`.
    /// `none` has no error branch to land in.
    #[error("`try` on an optional needs a `T?`-returning function, found `{return_ty}`")]
    TryAbsenceIntoResult { return_ty: Type, span: Span },
    /// ER47: bare `try` on a result inside a function that returns `T?`.
    #[error("`try` on a result needs an error branch to leave through, found `{return_ty}`")]
    TryErrorIntoOptional { return_ty: Type, span: Span },
    /// ER47: bare `try` on a flat `T? or E` — two branches could leave, so the
    /// composite has to say which is which.
    #[error("`try` on a `{found}` has two ways to leave")]
    TryOnFlatShape { found: Type, span: Span },
    /// ER14: `catch` is results-only.
    #[error("`catch` on an optional — absence carries no error")]
    CatchOnOptional { found: Type, span: Span },
    /// ER12: `?` is optionals-only.
    #[error("`?` on a result — `?` marks absence, not failure")]
    PresenceTestOnResult { found: Type, span: Span },
    /// ER12: `??` is optionals-only.
    #[error("`??` on a result — `?` marks absence, not failure")]
    CoalesceOnResult { found: Type, span: Span },
    /// OPT3/OPT11, and ER12 from the other side: `??` supplies the branch a
    /// `T?` doesn't have. On a value that is always there, there's no branch to
    /// supply and no way to lower it.
    ///
    /// Caught here rather than left to unification, which used to unify the
    /// operand against a synthesized `T or _` and report the failure as a
    /// mismatch against that shape — "expected `i64`, found `i32 or _`", naming
    /// an error branch the program never had (#662).
    ///
    /// `from_index` marks the case worth its own advice: `m[k]` panics when the
    /// key is absent rather than handing back a `T?`, so reaching for `??`
    /// after it is the natural mistake and `.get(k)` is the answer.
    #[error("`??` on `{found}` — there's no absent branch to fall back to")]
    CoalesceOnNonOptional {
        found: Type,
        from_index: bool,
        /// The left operand, whose type is the reason this is an error.
        value_span: Span,
        /// The fallback, which is the dead code to delete.
        default_span: Span,
        span: Span,
    },
    /// `!` negates a `bool`. `T?` doesn't coerce to `T` (OPT5), so lifting `!`
    /// through an optional is rejected rather than guessed — on a `bool?` a
    /// reader can't tell "negate the payload" from "test for absence".
    #[error("`!` on `{found}` — negation doesn't reach through an optional")]
    NotOnOptional { found: Type, span: Span },
    /// CV1a/CV2: an integer that doesn't fit the position it's going into.
    /// Widening is implicit; anything that can lose a value has to name a policy.
    #[error("`{from}` doesn't fit in `{to}`")]
    NarrowingNeedsPolicy { from: Type, to: Type, span: Span },
    /// ORD4: arithmetic between a signed and an unsigned integer. Comparison is
    /// the one operator family that crosses signedness, because it has an
    /// obviously-correct answer; `u64 + i32` has no obviously-correct result type.
    #[error("`{op}` between `{left}` and `{right}` — one is signed, the other isn't")]
    MixedSignednessArithmetic { op: &'static str, left: Type, right: Type, span: Span },
    /// CV1a: int→float is never implicit, so an integer and a float can't meet
    /// under an arithmetic operator. It type-checked, and native answered with an
    /// integer — the float operand was dropped without a word (#816).
    #[error("`{op}` between `{left}` and `{right}` — one is an integer, the other a float")]
    IntFloatArithmetic { op: &'static str, left: Type, right: Type, span: Span },
    /// ER11: `T or E` (E ≠ none) only auto-wraps at `return`.
    #[error("`{value}` doesn't become a `{target}` here — auto-wrap is return-only")]
    NoAutoWrapOutsideReturn { value: Type, target: Type, span: Span },
    /// OPT13: `x!` extracts the payload of a `T?`. On a value that is always
    /// there, there is no payload to extract and nothing that could panic.
    #[error("`!` on `{found}` — there's no payload to force out")]
    ForceUnwrapOnNonOptional { found: Type, span: Span },
    /// OPT32: `take` needs a `T?` place.
    #[error("`take` needs an optional slot, found `{found}`")]
    TakeOnNonOptional { found: Type, span: Span },
    /// OPT32: `take` empties the slot, so the slot has to be writable.
    #[error("`take` needs a mutable place")]
    TakeOnImmutablePlace { name: String, span: Span },
    /// std.api/SD4: the wrapper shapes have no methods; the operator replaces it.
    #[error("no method `{method}` on `{receiver}`")]
    WrapperMethodCut {
        method: String,
        receiver: Type,
        /// The operator spelling that does this job.
        fix: String,
        span: Span,
    },
    #[error("missing return statement")]
    MissingReturn {
        function_name: String,
        expected_type: Type,
        span: Span,
    },
    #[error("generic argument error: {0}")]
    GenericError(String, Span),
    #[error("cannot mutate `{var}` while borrowed")]
    AliasingViolation {
        var: String,
        borrow_span: Span,
        access_span: Span,
    },
    #[error("cannot mutate parameter `{name}`")]
    MutateReadOnlyParam {
        name: String,
        span: Span,
    },
    /// mem.pools/PF5: a write, insert, remove, or clear inside a
    /// `using frozen Pool<T>` context. `op` names the rejected operation.
    #[error("cannot {op} in a frozen `Pool<{elem}>` context")]
    FrozenContextWrite {
        op: String,
        elem: String,
        span: Span,
    },
    /// A required edge (`Link<T>`, no `?`) — needs batches to build and a delete
    /// policy to destroy, neither of which the prototype has.
    #[error("a required `Link<T>` edge is not supported yet — write `Link<T>?`")]
    NonOptionalLink {
        span: Span,
    },
    #[error("`{name}` is not a type any more — it's a strategy on `Shared`")]
    RetiredBoxType {
        name: String,
        replacement: String,
        span: Span,
    },
    #[error("cannot mutate `{name}` — declared `let`")]
    MutateConst {
        name: String,
        span: Span,
    },
    #[error("cannot mutate `{name}` — bound from a shared read lock")]
    MutateWithBinding {
        name: String,
        span: Span,
    },
    /// OPT19 / std.iteration/I1: a name a test or a pattern introduced. Same
    /// immutability as `let`, but "add `mut`" isn't writable at any of these
    /// sites, so the remedy comes from `from`.
    #[error("cannot mutate `{name}` — it's a binding, not a slot")]
    MutateBoundName {
        name: String,
        from: crate::checker::BoundFrom,
        span: Span,
    },
    #[error("`string` has no `{method}` — strings are immutable")]
    StringIsImmutable {
        method: String,
        span: Span,
    },
    #[error("`string.new()` doesn't exist — an empty string is `\"\"`")]
    StringNewRemoved {
        span: Span,
    },
    #[error("string slices are temporary — cannot store `{view_var}`")]
    StringSliceStored {
        source_var: String,
        /// The slicing expression as the user wrote it (`line.trim()`,
        /// `line[0..4]`). The message quotes this — describing the code in
        /// terms of a `line[i..j]` the program never contained meant the
        /// reader had to already know that `trim()` returns a slice (#694).
        ///
        /// `None` when the expression won't reprint exactly. Only an exact
        /// quote goes in, since anything else reads as the user's own code:
        /// rendering `lines[0]` as `lines[..]` produced a fix that doesn't
        /// compile, and `s[0..=4]` as `s[0..4]` named a shorter substring.
        slice_expr: Option<String>,
        /// True for the methods that hand back a sequence of views
        /// (`split`, `lines`, `chars`) rather than one. `.to_string()` is the
        /// fix for a single view and nonsense for a sequence, so the message
        /// can't offer the same one for both.
        yields_sequence: bool,
        view_var: String,
        slice_span: Span,
        store_span: Span,
    },
    #[error("cannot hold view from growable source `{source_var}`")]
    VolatileViewStored {
        source_var: String,
        view_var: String,
        source_span: Span,
        store_span: Span,
    },
    /// A `with` guard's bare identifier used as the block's own produced
    /// value, where the payload is a struct/enum/union. Boxes hand out no
    /// guards — the payload is reachable only inside the block (mem.boxes,
    /// "Why scoped access, not guards"). Scalars and `string` aren't checked
    /// here: copying them out is already an independent value (#559).
    #[error("the `with` guard `{name}` can't leave its block")]
    WithGuardEscapes {
        name: String,
        type_name: String,
        span: Span,
    },
    /// tool.warnings/W9 (`torn_lock_update`, W0907): a `with` block over a sync
    /// box assigns two or more fields of the locked value without `staged()`.
    ///
    /// A warning, not an error — partial state is sometimes harmless, and Rask
    /// has no poisoning to make it loud. But `ctrl.panic/LK3` means a panic
    /// between those two writes leaves survivors a half-done update, and
    /// `staged()` is the by-construction fix, so the sites that need it get
    /// pointed at it rather than left to find it.
    #[error("multi-field update under a lock without staged()")]
    TornLockUpdate {
        binding: String,
        box_name: String,
        first_field: String,
        second_field: String,
        first_span: Span,
        second_span: Span,
    },

    /// conc.sync/ST1: `staged()` is the source of a `with` binding and nothing
    /// else. `read`/`write` also have an expression-scoped form (R5); staged has
    /// none, because the commit needs a block boundary to happen at.
    #[error("`staged()` only works as the source of a `with` block")]
    StagedOutsideWith {
        name: String,
        span: Span,
    },

    /// conc.sync/ST3a: `staged()` under the `Local` strategy. There is no other
    /// task to observe a torn update and no unwind boundary to protect against,
    /// so the clone would buy nothing and cost a copy.
    #[error("`staged()` has nothing to protect under `Local`")]
    StagedOnLocal {
        name: String,
        span: Span,
    },

    /// conc.sync/R4: bare `with shared as v` — the lock has to be named.
    #[error("`with {name} as {binding}` doesn't say which lock")]
    BareSharedWith {
        name: String,
        binding: String,
        span: Span,
    },
    #[error("cannot mutate `{source_var}` while viewed by `{view_var}`")]
    MutateBorrowedSource {
        source_var: String,
        view_var: String,
        borrow_span: Span,
        mutate_span: Span,
    },
    #[error("heap allocation in @no_alloc function: {reason}")]
    NoAllocViolation {
        reason: String,
        function_name: String,
        span: Span,
    },
    #[error("guard pattern 'else' block must diverge (return, panic, etc), found {found}")]
    GuardElseMustDiverge {
        found: Type,
        span: Span,
    },
    #[error("parameter `{param_name}` requires `own` annotation at call site")]
    MissingOwnAnnotation {
        param_name: String,
        param_index: usize,
        span: Span,
    },
    #[error("unexpected `{annotation}` annotation for parameter `{param_name}`")]
    UnexpectedAnnotation {
        annotation: String,
        param_name: String,
        param_index: usize,
        span: Span,
    },
    /// PM4: an argument going into a `mutate` parameter is written
    /// `mutate arg`. Method receivers are exempt.
    #[error("`{callee}` can delete from `{arg}` — say `deleting` at the call site")]
    MissingDeletingMarker {
        callee: String,
        arg: String,
        param_name: String,
        span: Span,
    },
    /// PM4: an argument going into a `mutate` parameter is written
    /// `mutate arg`. Method receivers are exempt.
    #[error("`{callee}` mutates `{arg}` — mark it at the call site")]
    MissingMutateMarker {
        callee: String,
        arg: String,
        param_name: String,
        span: Span,
    },
    #[error("`try` requires a Result or Option type, found {found}")]
    TryOnNonResult {
        found: Type,
        span: Span,
    },
    #[error("`{found}` can't be iterated")]
    NotIterable {
        found: Type,
        span: Span,
    },
    #[error("{operation} requires `unsafe` block")]
    UnsafeRequired {
        operation: String,
        span: Span,
    },
    #[error("method `{method}` returns Self and cannot be called through `any {trait_name}`")]
    TraitObjectSelfReturn {
        trait_name: String,
        method: String,
        span: Span,
    },
    #[error("generic method `{method}` cannot be called through `any {trait_name}`")]
    TraitObjectGenericMethod {
        trait_name: String,
        method: String,
        span: Span,
    },
    #[error("`{ty}` does not implement `{trait_name}`")]
    TraitNotSatisfied {
        ty: String,
        trait_name: String,
        /// Where the requirement came from. The advice differs completely: a
        /// failed `as any Trait` is fixed by implementing the trait, a failed
        /// generic bound is usually fixed by passing a different type, and a
        /// `Numeric`/`Integer` bound can't be implemented at all. One message
        /// for all three told everyone to "implement `Integer` for `Marker`"
        /// and explained itself in terms of trait objects (#713).
        context: TraitBoundContext,
        span: Span,
    },
    /// A bound, conformance header or cast naming a trait that doesn't exist.
    ///
    /// Used to be reported as `TraitNotSatisfied` with `_` standing in for the
    /// type, because an unknown trait has no type to blame — so a typo in a
    /// bound read as a mysterious failure of the type system rather than as a
    /// name nobody had declared.
    #[error("no trait named `{trait_name}`")]
    NoSuchTrait {
        trait_name: String,
        /// Declared trait names, for a did-you-mean.
        known: Vec<String>,
        span: Span,
    },

    /// std.encoding/E12: an `Encode`/`Decode` bound that fails because of a
    /// specific field. Separate from `TraitNotSatisfied` because the advice is
    /// different — these markers are derived from the shape, not written out,
    /// so the fix is to change the field, and the message has to name it.
    #[error("`{ty}` cannot be {verb}")]
    NotSerializable {
        ty: String,
        trait_name: String,
        /// `verb` reads in the message: "encoded" or "decoded".
        verb: String,
        /// Dotted path to the offending field, when one can be pinned down.
        field: Option<String>,
        field_ty: Option<String>,
        span: Span,
    },

    /// std.encoding/E13a: a field the wire form leaves out with no default to
    /// build it from on decode.
    #[error("`{ty}` cannot be decoded: field `{field}` is left out of the wire form and has no default")]
    ExcludedFieldNeedsDefault {
        ty: String,
        field: String,
        span: Span,
    },

    #[error("the `+` operator cannot be used on strings")]
    StringAddForbidden {
        span: Span,
    },

    /// type.generics/DT1: `duck trait` is scratchpad-only — it can't be public.
    #[error("`duck trait {name}` cannot be public")]
    PublicDuckTrait {
        name: String,
        span: Span,
    },

    /// type.aliases/T9: nominal type used where underlying expected, or vice versa
    #[error("nominal type mismatch: expected `{expected}`, found `{found}`")]
    NominalMismatch {
        expected: Type,
        found: Type,
        nominal_name: String,
        span: Span,
    },

    /// ER21: public function uses `or _` (must declare error types explicitly)
    #[error("public function `{function_name}` must declare error types explicitly")]
    PublicInferredError {
        function_name: String,
        span: Span,
    },

    #[error("non-exhaustive match: missing variants {missing:?}")]
    NonExhaustiveMatch {
        missing: Vec<String>,
        span: Span,
    },

    #[error("undefined name `{name}`")]
    UndefinedName {
        name: String,
        span: Span,
    },

    #[error("unknown context `{name}` in `using` block")]
    UnknownContext {
        name: String,
        span: Span,
    },

    /// CC1: `using Multitasking`/`ThreadPool` may not appear on a function signature
    #[error("`using {ctx}` cannot appear on a function signature")]
    SignatureRuntimeContext {
        ctx: String,
        span: Span,
    },

    /// CC11: the entry point can't declare a context — it has no caller to supply one
    #[error("`{entry}` cannot declare a `using` context")]
    EntryPointContext {
        /// Entry function name, so an `@entry`-marked function reads correctly.
        entry: String,
        /// Alias the clause named, if any — the suggested local reuses it.
        alias: Option<String>,
        /// Context type as written: `Pool<Player>`.
        ty: String,
        span: Span,
    },

    /// conc.sync/SH7: a task-local `Shared` sent to another task.
    #[error("this `Shared` is task-local and cannot be sent")]
    LocalSharedSent {
        name: String,
        span: Span,
    },

    /// conc.sync/SH2: two `Shared` boxes with different strategies met.
    ///
    /// Not a deferrable obligation like most type mismatches — the strategy
    /// picks which lock the accessors take, so getting it wrong deadlocks
    /// rather than misbehaving visibly (#960).
    #[error("this box uses the `{found}` strategy, but `{expected}` is expected here")]
    SharedStrategyMismatch {
        found: String,
        expected: String,
        span: Span,
    },

    /// CC1: `spawn` used outside any `using Multitasking` block
    #[error("`spawn` must be inside a `using Multitasking {{ ... }}` block")]
    SpawnOutsideBlock {
        span: Span,
    },

    /// T6: cyclic type alias
    #[error("cyclic type alias: {cycle}")]
    CyclicTypeAlias {
        cycle: String,
        span: Span,
    },

    /// V5: private field accessed outside extend block
    #[error("field `{field}` on `{ty}` is private")]
    PrivateFieldAccess {
        ty: String,
        field: String,
        span: Span,
    },

    /// FD4: struct literal omits a field that has no default value
    #[error("missing field(s) in `{ty}` initializer")]
    MissingFields {
        ty: String,
        fields: Vec<String>,
        span: Span,
    },

    /// S1: a struct or enum name used as a function — `TaskId(1)`. Only a
    /// nominal type (`type Name = U`) has a `Name(value)` constructor (T7).
    #[error("`{name}` is {kind}, so calling it doesn't construct one")]
    TypeCalledAsFunction {
        name: String,
        /// Noun phrase with its article: "a struct" or "an enum".
        kind: String,
        /// Struct field names, so the fix can show the literal form.
        fields: Vec<String>,
        span: Span,
    },

    /// GC5: public function missing type annotation
    #[error("public function `{function_name}` requires explicit type annotations")]
    PublicMissingAnnotation {
        function_name: String,
        params: Vec<String>,
        missing_return: bool,
        span: Span,
    },

    /// D2: discard on Copy type (warning)
    #[error("`discard {name}` on Copy type `{ty}` has no effect")]
    DiscardCopyType {
        name: String,
        ty: Type,
        span: Span,
    },

    /// D3: discard on @resource type (error)
    #[error("cannot `discard` resource `{name}` — use its consuming method instead")]
    DiscardResourceType {
        name: String,
        ty: Type,
        span: Span,
    },

    /// D1: use after discard
    #[error("use of discarded value: `{name}`")]
    UseAfterDiscard {
        name: String,
        discarded_at: Span,
        span: Span,
    },

    /// SP3: zero step on range
    #[error("zero step")]
    ZeroStep {
        span: Span,
    },

    /// SP1/SP2: step direction doesn't match range direction (warning)
    #[error("step direction mismatch — range will be empty")]
    StepDirectionMismatch {
        range_span: Span,
        step_span: Span,
        /// "ascending" or "descending"
        range_direction: String,
        /// "positive" or "negative"
        step_direction: String,
    },


    /// E5/R5/MX3: standalone sync access without chaining
    #[error("standalone `.{method}()` on `{ty}` must be chained — use `.{method}().field` or `with` block")]
    BareSyncAccess {
        ty: String,
        method: String,
        span: Span,
    },

    /// E16: mixed explicit and auto-indexed discriminants
    #[error("enum `{enum_name}`: if any variant has `= N`, all must")]
    MixedDiscriminants {
        enum_name: String,
        span: Span,
    },

    /// E17: explicit discriminant on variant with fields
    #[error("enum `{enum_name}`: variant `{variant}` cannot have both fields and an explicit discriminant")]
    DiscriminantWithPayload {
        enum_name: String,
        variant: String,
        span: Span,
    },

    /// E15: duplicate discriminant value
    #[error("enum `{enum_name}`: duplicate discriminant value {value} on `{first}` and `{second}`")]
    DuplicateDiscriminant {
        enum_name: String,
        value: i128,
        first: String,
        second: String,
        span: Span,
    },

    /// E24: `@tag` needs a named payload to flatten into the tagged object
    #[error("enum `{enum_name}`: `@tag(\"{tag}\")` needs named payloads, but variant `{variant}` has an unnamed one")]
    TagOnUnnamedPayload {
        enum_name: String,
        variant: String,
        tag: String,
        span: Span,
    },

    /// E24: `@tag`'s key collides with one of a variant's own payload fields
    #[error("enum `{enum_name}`: `@tag(\"{tag}\")` collides with field `{tag}` on variant `{variant}`")]
    TagCollidesWithField {
        enum_name: String,
        variant: String,
        tag: String,
        span: Span,
    },

    /// ER3: success and error types in `T or E` must be distinct
    #[error("`T or E` requires T and E to be distinct types — both sides are `{ty}`")]
    ResultNotDisjoint {
        ty: Type,
        span: Span,
    },

    /// ER3a: a generic's `T or E` collapsed to `E or E` at this call site.
    /// `param` is the type parameter, `arg` what it was bound to, `other` the
    /// spelling of the branch it collided with.
    #[error("`{param}` may not be `{arg}` here — it would collide with the `{other}` branch of `{callee}`")]
    ResultNotDisjointAtInstantiation {
        callee: String,
        param: String,
        arg: Type,
        other: Type,
        span: Span,
    },

    /// ER4: error type must implement `Error` — `message(self) -> string`.
    #[error("error type `{ty}` must implement `Error` — needs `func message(self) -> string`")]
    ErrorTraitMissing {
        ty: Type,
        span: Span,
    },

    /// U5 / nested-optional: a sum type cannot contain the same variant twice.
    /// Covers `T??` (= `(T or none) or none`), `(T or E) or E`, and similar.
    #[error("duplicate variant `{variant}` in sum type `{ty}`")]
    DuplicateSumVariant {
        ty: Type,
        variant: Type,
        span: Span,
    },

    /// ctrl.ensure/ER4 (body) and ER3 (`else` handler): `try` has nowhere to
    /// propagate to from cleanup code. `region` names which of the two it is.
    #[error("`try` can't be used {region}")]
    TryInEnsure {
        region: &'static str,
        span: Span,
    },

    /// ER22: `else as e` requires a `T or E` condition to bind the error
    #[error("`else as {name}` requires an `if r?` condition on a Result (`T or E`)")]
    ElseBindingNotResult {
        name: String,
        span: Span,
    },

    /// ER23: `is TypeName as ...` requires the scrutinee to be a Result
    #[error("type pattern `{ty_name}` requires a Result scrutinee, found `{found}`")]
    TypePatternNotResult {
        ty_name: String,
        found: Type,
        span: Span,
    },

    /// ER23: `is TypeName` must reference a component of the union error
    #[error("type pattern `{ty_name}` is not part of the error union `{union}`")]
    TypePatternNotInUnion {
        ty_name: String,
        union: Type,
        span: Span,
    },

    /// E19/E21: a serialization annotation the compiler can't act on — the old
    /// `@skip` spelling, or `@rename` given something that isn't a string.
    #[error("`@{attr}` on field `{field}`: {problem}")]
    BadFieldAnnotation {
        attr: String,
        field: String,
        problem: String,
        fix: String,
        span: Span,
    },

    /// type.annotations/AN1-AN5: a user annotation declared or attached wrong —
    /// reserved name, bad field type, wrong target, unknown/missing/duplicate
    /// argument.
    #[error("annotation `{name}`: {problem}")]
    BadAnnotation {
        name: String,
        problem: String,
        fix: String,
        /// Which annotation rule this violates, in words. One shared reason
        /// can't cover "you can't construct one" and "that's not a type" —
        /// each site says why its own rule exists.
        why: &'static str,
        span: Span,
    },

    /// PS2: package-level mutable state goes behind a sync box. A bare `const`
    /// collection is one instance every task can reach, so writing to it from
    /// two of them is a data race out of safe code.
    #[error("`{name}` is package-level state — writing to it needs a sync box")]
    MutatePackageState {
        name: String,
        ty: String,
        span: Span,
    },

    /// `@allow(name)` where nothing answers to `name` — a typo, or a rule id
    /// that doesn't exist. Silence here is indistinguishable from a warning
    /// correctly suppressed, so it's an error.
    #[error("`@allow({name})` names nothing")]
    UnknownAllowName {
        name: String,
        suggestion: Option<String>,
        span: Span,
    },

    /// OPT2/ER2: legacy `Some(x)`/`Ok(x)`/`Err(x)` constructor — migration error
    #[error("`{name}(...)` is no longer a valid constructor")]
    LegacyWrapperConstructor {
        name: String,
        span: Span,
    },

    /// OPT2/ER2: legacy `Ok`/`Err`/`Some`/`None` pattern — migration error.
    /// `Ok`/`Err`/`Some`/`None` are not pattern names in the no-wrapper spec.
    #[error("`{name}` is not a pattern — use the operator or type-pattern form")]
    LegacyWrapperPattern {
        name: String,
        /// True if the pattern had parens (e.g. `Ok(v)` vs bare `Ok`).
        with_binding: bool,
        span: Span,
    },

    /// OPT NO_MATCH: match on `T?` is rejected — migration error
    #[error("match on an Option is not supported — use the `?`-operator family")]
    MatchOnOption {
        span: Span,
    },

    /// type.primitives CV1–CV4, CH5, BL3: an `as` cast that isn't lossless
    /// widening. `class` selects the diagnostic + suggested conversion form.
    #[error("invalid `as` cast from {src_ty} to {dst_ty}")]
    InvalidCast {
        src_ty: Type,
        dst_ty: Type,
        /// Original target spelling (e.g. `usize`), for the suggested fix.
        target_name: String,
        class: InvalidCastClass,
        span: Span,
    },

    /// `as` to a target that is neither a number nor a trait object. There is
    /// no third meaning for `as`, and accepting one silently let
    /// `[1, 2, 3] as Vec<i64>` through as a pointer reinterpretation (#862).
    #[error("`as` doesn't convert to `{target_name}`")]
    AsCastNotConvertible {
        src_ty: Type,
        /// Original target spelling, for the message and the suggested fix.
        target_name: String,
        span: Span,
    },

    /// type.primitives CV5–CV10: a conversion form applied to the wrong
    /// source/target kind (e.g. `floor` on an integer).
    #[error("invalid conversion: {message}")]
    InvalidConvert {
        message: String,
        span: Span,
    },

    /// An integer literal whose value doesn't fit the type it ended up with.
    /// Codegen would keep the low bits, so `let b: u8 = 300` printed 300 in
    /// the interpreter and 44 natively.
    #[error("integer literal {literal} is out of range for `{ty}`")]
    IntLiteralOutOfRange {
        /// The literal as written (already signed, with `-` folded in).
        literal: String,
        ty: Type,
        /// Inclusive bounds of `ty`, for the message.
        min: String,
        max: String,
        span: Span,
    },

    /// mem.resource-types/RC1, RC3: a `Vec<T>` or `Map<K, V>` element (or key)
    /// is a linear value (`@resource`, transitively-linear, or an optional/
    /// tuple/array built from one). Vec/Map drop can't consume linear elements,
    /// so the type is rejected. `container` is "Vec" or "Map".
    #[error("`{container}` cannot hold linear value `{elem}`")]
    LinearInContainer {
        container: String,
        elem: Type,
        span: Span,
    },

    /// type.sequence/SEQ29: `to_map` is defined only on a sequence of pairs.
    #[error("`to_map` needs a sequence of pairs")]
    ToMapNeedsPairs {
        elem: Type,
        span: Span,
    },

    /// type.generics/HA1, HA4: a Map key has to be Hashable.
    #[error("`{key}` can't key a Map")]
    UnhashableMapKey {
        key: Type,
        /// Which way out to offer — the advice differs per kind of type.
        fix: MapKeyFix,
        span: Span,
    },

    /// mem.atomics/GA2: `Atomic<T>` needs a payload the hardware can treat as
    /// one word. The reason is carried so the message can say which rule the
    /// payload broke rather than restating the rule.
    #[error("`Atomic<{ty}>` — {reason}")]
    AtomicPayload {
        ty: Type,
        reason: String,
        span: Span,
    },

    /// ctrl.comptime/CT53: `value.(expr)` is rewritten to a direct field access
    /// while compiling, so the name has to be one the compiler knows. A runtime
    /// string has nothing to rewrite to.
    #[error("the field name in `value.(…)` isn't known at compile time")]
    DynamicFieldNameNotComptime {
        span: Span,
    },

    /// std.collections/V1, mem.pools/PL4 (#310): an index expression `c[i]`
    /// whose index type doesn't match what the container accepts.
    #[error("cannot index {container} with {found}")]
    IndexTypeMismatch {
        /// The container being indexed (for the message).
        container: Type,
        /// The index expression's type.
        found: Type,
        /// What the container actually accepts.
        kind: IndexErrorKind,
        span: Span,
    },

    /// std.collections (#901): a length-changing method called on `[T; N]`.
    /// The length is part of the type, so there is nowhere for the element to
    /// go — and the `Vec` method table used to answer these calls anyway.
    #[error("`{method}` doesn't exist on a fixed array")]
    FixedArrayGrowth {
        /// The method that was called — named in the message and the fix.
        method: String,
        /// The receiver's type, so the message can print its length.
        array: Type,
        span: Span,
    },
}

/// What went wrong at an index site — drives the E0819 diagnostic.
#[derive(Debug, Clone, PartialEq)]
pub enum IndexErrorKind {
    /// Vec/array/slice/string are position-indexed: the index must be an integer.
    ExpectedInteger,
    /// `Map<K, V>` is indexed by `K` (carried).
    ExpectedKey(Type),
    /// `Pool<T>` is indexed by its handle. Carries the expected `Handle<T>`.
    ExpectedHandle(Type),
    /// A range was used to slice a container that isn't sliceable (Map, Pool).
    NotSliceable,
}

/// Where a trait requirement came from — drives the advice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraitBoundContext {
    /// `value as any Trait` — the box needs a vtable, so the concrete type has
    /// to have the methods.
    TraitObjectCast,
    /// `f<T: Trait>(…)` at a call site — the type argument doesn't qualify.
    GenericBound,
    /// `extend T with Trait { … }` — the block claims a conformance it doesn't
    /// deliver.
    ConformanceHeader,
    /// A bound on one of the numeric traits (NT1–NT3). These are sets of
    /// primitive types rather than method lists, so "implement it" is not
    /// advice anyone can act on.
    NumericBound,
}

/// Why an `as` cast is rejected — drives the diagnostic and suggested fix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvalidCastClass {
    /// CV2: narrowing int→int (target range smaller).
    Narrowing,
    /// CV3: same-width sign reinterpretation (i32→u32).
    SignReinterpret,
    /// CV4: float→int via `as`.
    FloatToInt,
    /// CV4-adjacent: float→float narrowing (f64→f32).
    FloatNarrowing,
    /// CH5: integer→char via `as`.
    IntToChar,
    /// BL3: any conversion involving bool.
    Bool,
    /// Fallback: char/other lossy conversion with no obvious form.
    Other,
}

impl TypeError {
    /// Rewrite every type this error carries.
    ///
    /// Diagnostics print a `Type`, and `Type::Named(id)` carries no name — the
    /// name lives in the checker's type table. Doing that lookup at each of the
    /// 126 places an error gets built is discipline nobody keeps, and nobody did:
    /// the error side of a `T or E` printed as an internal id in at least three
    /// unrelated messages (#646). One walk, applied once on the way out of the
    /// checker, covers every site — including the ones not written yet.
    ///
    /// Deliberately exhaustive with no catch-all. A new variant that carries a
    /// type won't compile until it's listed here, which is the only thing that
    /// keeps this from drifting back.
    pub(crate) fn map_types(&mut self, f: &dyn Fn(&Type) -> Type) {
        use TypeError::*;
        match self {
            DiscardCopyType { ty, .. }
            | DiscardResourceType { ty, .. }
            | ErrorTraitMissing { ty, .. }
            | InfiniteType { ty, .. }
            | IntLiteralOutOfRange { ty, .. }
            | NoSuchField { ty, .. }
            | NoSuchMethod { ty, .. }
            | NotCallable { ty, .. }
            | ResultNotDisjoint { ty, .. } => *ty = f(ty),

            FixedArrayGrowth { array, .. } => *array = f(array),

            IncomparableOperands { left, right, .. } => {
                *left = f(left);
                *right = f(right);
            }

            AtomicPayload { ty, .. } => *ty = f(ty),

            CatchOnOptional { found, .. }
            | CoalesceOnNonOptional { found, .. }
            | CoalesceOnResult { found, .. }
            | ForceUnwrapOnNonOptional { found, .. }
            | GuardElseMustDiverge { found, .. }
            | NotIterable { found, .. }
            | NotOnOptional { found, .. }
            | PresenceTestOnResult { found, .. }
            | TakeOnNonOptional { found, .. }
            | TryOnFlatShape { found, .. }
            | TryOnNonResult { found, .. }
            | TypePatternNotResult { found, .. } => *found = f(found),

            TryAbsenceIntoResult { return_ty, .. }
            | TryErrorIntoOptional { return_ty, .. }
            | TryInNonPropagatingContext { return_ty, .. } => *return_ty = f(return_ty),

            WrapperMethodCut { receiver, .. } => *receiver = f(receiver),

            MissingReturn { expected_type, .. } => *expected_type = f(expected_type),

            TypePatternNotInUnion { union, .. } => *union = f(union),

            LinearInContainer { elem, .. } => *elem = f(elem),
            UnhashableMapKey { key, .. } => *key = f(key),
            ToMapNeedsPairs { elem, .. } => *elem = f(elem),

            DuplicateSumVariant { ty, variant, .. } => {
                *ty = f(ty);
                *variant = f(variant);
            }

            IndexTypeMismatch { container, found, .. } => {
                *container = f(container);
                *found = f(found);
            }

            InvalidCast { src_ty, dst_ty, .. } => {
                *src_ty = f(src_ty);
                *dst_ty = f(dst_ty);
            }

            AsCastNotConvertible { src_ty, .. } => {
                *src_ty = f(src_ty);
            }

            Mismatch { expected, found, .. } => {
                *expected = f(expected);
                *found = f(found);
            }

            NarrowingNeedsPolicy { from, to, .. } => {
                *from = f(from);
                *to = f(to);
            }

            MixedSignednessArithmetic { left, right, .. } => {
                *left = f(left);
                *right = f(right);
            }

            IntFloatArithmetic { left, right, .. } => {
                *left = f(left);
                *right = f(right);
            }

            NoAutoWrapOutsideReturn { value, target, .. } => {
                *value = f(value);
                *target = f(target);
            }

            NominalMismatch { expected, found, .. } => {
                *expected = f(expected);
                *found = f(found);
            }

            ResultNotDisjointAtInstantiation { arg, other, .. } => {
                *arg = f(arg);
                *other = f(other);
            }

            // Carry no types.
            Undefined(..)
            | DynamicFieldNameNotComptime { .. }
            | UnresolvedType { .. }
            | ArityMismatch { .. }
            | UnimplementedStdlibMethod { .. }
            | NotDisplayable { .. }
            | UnboundedTypeParamMethod { .. }
            | CannotInfer { .. }
            | InvalidTypeString(..)
            | UnknownTypeName { .. }
            | SingleLetterTypeName { .. }
            | TryErrorMismatch { .. }
            | AmbiguousErrorWrap { .. }
            | TryOutsideFunction { .. }
            | TakeOnImmutablePlace { .. }
            | GenericError(..)
            | AliasingViolation { .. }
            | MutateReadOnlyParam { .. }
            | FrozenContextWrite { .. }
            | MutateConst { .. }
            | RetiredBoxType { .. }
            | LocalSharedSent { .. }
            | SharedStrategyMismatch { .. }
            | NonOptionalLink { .. }
            | MutateWithBinding { .. }
            | MutateBoundName { .. }
            | StringIsImmutable { .. }
            | StringNewRemoved { .. }
            | StringSliceStored { .. }
            | VolatileViewStored { .. }
            | WithGuardEscapes { .. }
            | BareSharedWith { .. }
            | StagedOnLocal { .. }
            | StagedOutsideWith { .. }
            | TornLockUpdate { .. }
            | MutateBorrowedSource { .. }
            | NoAllocViolation { .. }
            | MissingOwnAnnotation { .. }
            | UnexpectedAnnotation { .. }
            | MissingDeletingMarker { .. }
            | MissingMutateMarker { .. }
            | UnsafeRequired { .. }
            | TraitObjectSelfReturn { .. }
            | TraitObjectGenericMethod { .. }
            | TraitNotSatisfied { .. }
            | NoSuchTrait { .. }
            | NotSerializable { .. }
            | ExcludedFieldNeedsDefault { .. }
            | StringAddForbidden { .. }
            | PublicDuckTrait { .. }
            | PublicInferredError { .. }
            | NonExhaustiveMatch { .. }
            | UndefinedName { .. }
            | UnknownContext { .. }
            | SignatureRuntimeContext { .. }
            | EntryPointContext { .. }
            | SpawnOutsideBlock { .. }
            | CyclicTypeAlias { .. }
            | PrivateFieldAccess { .. }
            | MissingFields { .. }
            | TypeCalledAsFunction { .. }
            | PublicMissingAnnotation { .. }
            | UseAfterDiscard { .. }
            | ZeroStep { .. }
            | StepDirectionMismatch { .. }
            | BareSyncAccess { .. }
            | BadFieldAnnotation { .. }
            | BadAnnotation { .. }
            | UnknownAllowName { .. }
            | MutatePackageState { .. }
            | MixedDiscriminants { .. }
            | DiscriminantWithPayload { .. }
            | DuplicateDiscriminant { .. }
            | TagOnUnnamedPayload { .. }
            | TagCollidesWithField { .. }
            | ElseBindingNotResult { .. }
            | TryInEnsure { .. }
            | LegacyWrapperConstructor { .. }
            | LegacyWrapperPattern { .. }
            | MatchOnOption { .. }
            | InvalidConvert { .. } => {}
        }
    }
}

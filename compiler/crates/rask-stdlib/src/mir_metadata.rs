// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! MIR-level metadata derived from stdlib stub files.
//!
//! Carries each stdlib function's return type verbatim, plus a coarse reading
//! of it for the two questions that aren't about layout. Keeps the stub files
//! as the single source of truth for stdlib API shapes.

use rask_ast::ty::TypeExpr;
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use crate::stubs::StubRegistry;

/// A coarse reading of a stub's return type.
///
/// No longer a type: MIR resolves `ret_ty` with `resolve_type_str`, the one
/// parser that knows `f32` from `f64`, transparent aliases, arrays, unions,
/// generic instantiations, and which named stdlib types are word-sized runtime
/// handles. This answers the two questions that aren't about layout —
/// `names_a_type_param` and `ret_type_prefix` — and nothing else reads it.
///
/// It used to answer the layout question too, and got two things wrong that no
/// caller could see: `f32` and `f64` shared a variant, and every named stdlib
/// type came back `i64`, which is right for a handle and wrong for anything
/// else (`StringView` needed a hard-coded exception). See #1025.
#[derive(Debug, Clone, PartialEq)]
pub enum RetCategory {
    Void,
    Bool,
    I64,
    /// An integer at a width other than 64-bit-signed, spelled as its Rask name.
    ///
    /// Every integer return used to collapse to `I64`. Mostly invisible, because
    /// most of them are lengths and counts — but a value that spans its range
    /// renders as the signed reading of its bits: `string.hash()` is `u64` and
    /// FNV-1a fills the range, so half of its answers printed negative (#823).
    Int(IntWidth),
    F64,
    String,
    /// A `char` — 4 bytes, not 8. Folding it into I64 gave `char?` an 8-byte
    /// payload slot while the rest of the compiler used 4, so a function
    /// forwarding `char_at`'s result read the payload from the wrong offset
    /// (#693).
    Char,
    Ptr,
    Option(Box<RetCategory>),
    Result {
        ok: Box<RetCategory>,
        err: Box<RetCategory>,
    },
    /// A named stdlib type (e.g., "File", "Vec", "Shared").
    Named(std::string::String),
    /// The stub's own type parameter — `Vec.remove` is declared `-> T`.
    ///
    /// This used to be recorded as `I64`, which is where "`T` reaches MIR as a
    /// bare i64" came from: a `Vec<string>.remove(i)` destination got an 8-byte
    /// slot, half the string was copied, and no refcount was taken on the
    /// payload. The stub genuinely doesn't know — only the call site does — so
    /// the honest answer is to say which parameter it is and let the caller ask
    /// something that knows (#1020).
    TypeParam(std::string::String),
    /// Tuple of types (e.g., `(Request, Responder)`).
    Tuple(Vec<RetCategory>),
}

impl RetCategory {
    /// Does this return type still mention one of the stub's type parameters?
    ///
    /// `Vec.remove` is `-> T` and `Vec.get` is `-> T?`; both are holes the call
    /// site has to fill. A caller that can't fill one is better off knowing that
    /// than being handed a width the stub invented.
    pub fn names_a_type_param(&self) -> bool {
        match self {
            RetCategory::TypeParam(_) => true,
            RetCategory::Option(inner) => inner.names_a_type_param(),
            RetCategory::Result { ok, err } => {
                ok.names_a_type_param() || err.names_a_type_param()
            }
            RetCategory::Tuple(elems) => elems.iter().any(|e| e.names_a_type_param()),
            _ => false,
        }
    }
}

/// An integer width other than the `I64` default, by its Rask name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntWidth {
    I8, I16, I32, I128,
    U8, U16, U32, U64, U128,
    /// `usize`/`isize` — pointer-sized, decided by `rask_ast::primitives`.
    Usize, Isize,
}

/// Metadata for a single stdlib method, derived from stubs.
#[derive(Debug, Clone)]
pub struct StdlibMethodMeta {
    /// Qualified name as MIR sees it: "Vec_push", "fs_open".
    pub qualified_name: std::string::String,
    /// Return type category. Down to one live question — does the stub's
    /// return type name one of the type's parameters? — now that MIR resolves
    /// the type string itself. See `ret_ty`.
    pub ret_category: RetCategory,
    /// The stub's return type as written.
    pub ret_ty: TypeExpr,
    /// Type prefix of the return value (for local_type_prefix tracking).
    /// E.g., "fs_open" returns File → prefix "File".
    pub ret_type_prefix: Option<std::string::String>,
    /// Declared `self`, in any mode — so MIR's argument zero is the receiver.
    pub takes_self: bool,
    /// Declared `take self` — the call consumes its receiver.
    pub take_self: bool,
    /// Which declared parameters are `take`, positionally. The receiver is not
    /// among them; see `keeps_argument` for the index shift.
    pub takes: Vec<bool>,
    /// Declared `@moves_out`: the result is an element taken out of the
    /// receiver, now the caller's.
    pub moves_out: bool,
    /// `@places_node`: the last argument is a node bound for a rack.
    pub places_node: bool,
    /// `@eager_callback`: the closure argument is run to completion and not kept.
    pub eager_callback: bool,
    /// The type whose `extend` block declares this method; `None` for a free
    /// function.
    pub owner: Option<std::string::String>,
    /// Which MIR arguments (receiver first) are declared as one of the type's
    /// own parameters: the `value: T` of `Atomic.store`. What the caller hands
    /// over there is the payload itself, at whatever type `T` is.
    pub payload_args: Vec<usize>,
}

/// MIR argument positions whose declared type is a bare type parameter.
fn payload_args(params: &[(std::string::String, TypeExpr)], takes_self: bool) -> Vec<usize> {
    params
        .iter()
        .enumerate()
        .filter(|(_, (_, ty))| matches!(ret_category(ty), RetCategory::TypeParam(_)))
        .map(|(i, _)| i + usize::from(takes_self))
        .collect()
}

/// Cached metadata derived from StubRegistry.
struct MetadataCache {
    type_names: HashSet<std::string::String>,
    module_names: HashSet<std::string::String>,
    method_metas: Vec<StdlibMethodMeta>,
    /// qualified_name → index into method_metas
    by_name: HashMap<std::string::String, usize>,
    /// `Type_method` for every method a stdlib type declares.
    method_symbols: HashSet<std::string::String>,
}

static CACHE: OnceLock<MetadataCache> = OnceLock::new();

fn build_cache() -> MetadataCache {
    let reg = StubRegistry::load();

    let mut type_names = HashSet::new();
    let mut module_names = HashSet::new();
    let mut method_metas = Vec::new();
    let mut method_symbols = HashSet::new();

    for type_name in reg.type_names() {
        // `fs`, `io`, `json` are namespaces of free functions; `string`,
        // `cstring`, `Vec` are types you can hold a value of. Capitalization
        // used to be the whole test, with `string` written in as the one
        // exception — so the second lowercase type, `cstring`, was filed as a
        // module and dispatch had no prefix to mangle `c.to_string()` with.
        //
        // The difference that actually matters is whether there is a receiver:
        // a module's functions all take their arguments, a type's methods take
        // `self`. Capitalization stays as the answer for a type that happens to
        // declare only constructors.
        let has_a_receiver = reg.methods(type_name).iter().any(|m| m.takes_self);
        let is_type = has_a_receiver
            || type_name.chars().next().map(|c| c.is_uppercase()).unwrap_or(false);
        if is_type {
            type_names.insert(type_name.to_string());
        } else {
            module_names.insert(type_name.to_string());
        }

        for method in reg.methods(type_name) {
            let qualified = format!("{}_{}", type_name, method.name);
            method_symbols.insert(qualified.clone());
            let ret_cat = ret_category(&method.ret_ty);
            let ret_prefix = ret_type_prefix(&ret_cat);
            method_metas.push(StdlibMethodMeta {
                qualified_name: qualified,
                ret_category: ret_cat,
                ret_ty: method.ret_ty.clone(),
                ret_type_prefix: ret_prefix,
                takes_self: method.takes_self,
                take_self: method.take_self,
                takes: method.param_modes.iter().map(|m| m.is_take).collect(),
                moves_out: method.effects.moves_out,
                places_node: method.effects.places_node,
                eager_callback: method.effects.eager_callback,
                owner: Some(type_name.to_string()),
                payload_args: payload_args(&method.params, method.takes_self),
            });
        }
    }

    // Top-level functions (println, print, etc. are builtins — skip them)
    for func in reg.functions() {
        let ret_cat = ret_category(&func.ret_ty);
        let ret_prefix = ret_type_prefix(&ret_cat);
        method_metas.push(StdlibMethodMeta {
            qualified_name: func.name.clone(),
            ret_category: ret_cat,
            ret_ty: func.ret_ty.clone(),
            ret_type_prefix: ret_prefix,
            takes_self: false,
            take_self: false,
            takes: func.param_modes.iter().map(|m| m.is_take).collect(),
            moves_out: func.effects.moves_out,
            places_node: func.effects.places_node,
            eager_callback: func.effects.eager_callback,
            owner: None,
            payload_args: payload_args(&func.params, false),
        });
    }

    let by_name = method_metas
        .iter()
        .enumerate()
        .map(|(i, m)| (m.qualified_name.clone(), i))
        .collect();

    MetadataCache {
        type_names,
        module_names,
        method_metas,
        by_name,
        method_symbols,
    }
}

fn cache() -> &'static MetadataCache {
    CACHE.get_or_init(build_cache)
}

// ── Public API ──────────────────────────────────────────────────

/// All stdlib type names (uppercase + "string").
pub fn stdlib_type_names() -> &'static HashSet<std::string::String> {
    &cache().type_names
}

/// All stdlib module names (lowercase except "string").
pub fn stdlib_module_names() -> &'static HashSet<std::string::String> {
    &cache().module_names
}

/// All method metadata entries.
pub fn method_metas() -> &'static [StdlibMethodMeta] {
    &cache().method_metas
}

/// Look up metadata for a specific qualified name.
pub fn lookup(qualified_name: &str) -> Option<&'static StdlibMethodMeta> {
    let idx = cache().by_name.get(qualified_name)?;
    Some(&cache().method_metas[*idx])
}

/// Does stdlib type `prefix` define a method `method`? Keyed on the stub API.
/// Whether `prefix.method` is declared `@unimplemented` — a signature with
/// nothing behind it on either backend.
///
/// Callers get a diagnostic at their call site instead of discovering it as
/// `Function not found: Vec_reserve` out of codegen, or as a runtime error
/// after the program has already started.
pub fn is_unimplemented(prefix: &str, method: &str) -> bool {
    StubRegistry::load()
        .get_type(prefix)
        .and_then(|t| t.methods.iter().find(|m| m.name == method))
        .is_some_and(|m| m.unimplemented)
}

/// Is `name` in the symbol namespace the stdlib owns?
///
/// A method `m` on `T` is the symbol `T_m` to the backends, and a program
/// function is its own name, so `func Vec_len` and `Vec.len` would be one
/// symbol (#1307). That covers every `T_x` the stdlib declares, and every one
/// lowering makes up for a stdlib type: `Vec_index` is `v[i]`, which no
/// declaration lists. So a name is taken when it is declared, or when it
/// starts with a stdlib type or a primitive and an underscore.
pub fn is_method_symbol(name: &str) -> bool {
    if cache().method_symbols.contains(name) {
        return true;
    }
    let Some((head, _)) = name.split_once('_') else { return false };
    cache().type_names.contains(head)
        || rask_ast::primitives::is_integer(head)
        || rask_ast::primitives::is_float(head)
        || MIR_FAMILIES.contains(&head)
}

/// Heads of the symbols lowering makes up that name no stdlib type: a
/// `Shared<T, Local>` is called through `Cell_*`, a field read that consumes
/// its slot is `Field_take`, and `Link<T>` has no stdlib file to declare
/// `Link_set`.
const MIR_FAMILIES: &[&str] = &["Cell", "Mutex", "Field", "Link"];

pub fn type_has_method(prefix: &str, method: &str) -> bool {
    cache().by_name.contains_key(&format!("{}_{}", prefix, method))
}

// ── What a call does with what it is handed ─────────────────────
//
// These were lists of method-name prefixes kept by hand in `rc_elide`,
// `rc_insert` and `container_drop`, then one table of spellings here. The
// declaration in `stdlib/*.rk` already says all of it, in the language's own
// words, so a stdlib method's shape is read from that. A call lowering makes
// up, which no declaration answers for, states its own shape where it is
// emitted. Nothing reads a call's spelling to find out.

/// What a call does with its receiver.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Receiver {
    /// There is none: a static constructor, or a plain function.
    Absent,
    /// Read or written in place. The caller still owns it afterwards.
    Borrowed,
    /// Taken: the call is the receiver's last use (`take self`, a free).
    Consumed,
}

/// What a call does with what it is handed and what it hands back: the facts
/// the ownership passes ask of every call.
///
/// Read from the declaration for a stdlib method
/// ([`CallShape::of_declaration`]), stated at the emit site for an operation
/// lowering makes up (`Vec_free`, a `with` block's acquire).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CallShape {
    receiver: Receiver,
    /// Bit `i`: argument `i` is kept or freed, so the caller has given it away.
    /// An argument past the declared parameters (an element size, a function
    /// address) carries nothing owned.
    given: u64,
    returns_a_view: bool,
    moves_out: bool,
    eager_callback: bool,
    fresh_release: Option<&'static str>,
}

impl CallShape {
    /// An operation that borrows its receiver, keeps none of its arguments and
    /// hands back something made fresh rather than pointed at inside the
    /// receiver: a read of a string, a write through a link, a rendering.
    pub const fn borrows_receiver() -> Self {
        Self {
            receiver: Receiver::Borrowed,
            given: 0,
            returns_a_view: false,
            moves_out: false,
            eager_callback: false,
            fresh_release: None,
        }
    }

    /// An operation that takes its receiver away: a free, or one end of a
    /// refcount given back. Nothing kept beyond it, nothing pointed into.
    pub const fn consumes_receiver() -> Self {
        Self { receiver: Receiver::Consumed, given: 1, ..Self::borrows_receiver() }
    }

    /// Not a method: a constructor or a plain function. No receiver to borrow,
    /// nothing kept, nothing pointed into.
    pub const fn no_receiver() -> Self {
        Self { receiver: Receiver::Absent, ..Self::borrows_receiver() }
    }

    /// A function with no receiver that gives these argument positions away
    /// and only reads the rest.
    pub fn giving(positions: &[usize]) -> Self {
        let given = positions.iter().filter(|p| **p < 64).fold(0u64, |bits, p| bits | 1 << p);
        Self { given, ..Self::no_receiver() }
    }

    /// The shape a stdlib declaration states: `take` parameters are given,
    /// `take self` consumes, and the result points into the receiver when its
    /// type names one of the receiver type's own parameters (`Vec.get -> T?`).
    pub fn of_declaration(m: &StdlibMethodMeta) -> Self {
        let receiver = match (m.takes_self, m.take_self) {
            (false, _) => Receiver::Absent,
            (true, true) => Receiver::Consumed,
            (true, false) => Receiver::Borrowed,
        };
        let first_param = usize::from(m.takes_self);
        let mut given = u64::from(m.takes_self && m.take_self);
        for (i, take) in m.takes.iter().enumerate() {
            if *take && i + first_param < 64 {
                given |= 1 << (i + first_param);
            }
        }
        // A `take self` method has no receiver left for the result to point
        // into: it consumed it, and what comes back is the caller's
        // (`Handle.join(take self) -> T or JoinError`, #1223). `@moves_out`
        // says a `mutate self` method hands its element over instead.
        let points_into_receiver = m.takes_self
            && !m.take_self
            && !m.moves_out
            && (m.ret_category == RetCategory::Ptr || m.ret_category.names_a_type_param());
        // The release of a handle the call hands back, when its declared
        // return type is one that frees itself (`@handle(release: "…")`) and
        // isn't a container, whose free also walks its elements:
        // `Random.new()`, `Rack.staged()`, `Atomic.new(v)`.
        let fresh_release = m
            .ret_ty
            .last_segment()
            .filter(|head| !matches!(*head, "Vec" | "Map" | "Rack"))
            .and_then(crate::handles::declared_release);
        Self {
            receiver,
            given,
            returns_a_view: points_into_receiver,
            moves_out: m.moves_out,
            eager_callback: m.eager_callback,
            fresh_release,
        }
    }

    /// This operation, but handing back a view into its receiver. For an
    /// operation lowering makes up that reads an element where it sits.
    pub const fn viewing(self) -> Self {
        Self { returns_a_view: true, ..self }
    }

    /// This operation, but running its closure argument to completion.
    pub const fn eager_callback(self) -> Self {
        Self { eager_callback: true, ..self }
    }

    pub fn receiver(&self) -> Receiver {
        self.receiver
    }

    /// Does the call keep or free the argument at `index`? MIR counts the
    /// receiver as argument zero.
    pub fn gives(&self, index: usize) -> bool {
        index < 64 && self.given & (1 << index) != 0
    }

    /// Does the result point into storage the receiver keeps owning? The caller
    /// may read it and must never release it: the container will.
    pub fn returns_a_view(&self) -> bool {
        self.returns_a_view
    }

    /// Is the result an element taken out of the receiver, now the caller's
    /// (`pop`, `remove`)?
    pub fn moves_out(&self) -> bool {
        self.moves_out
    }

    /// Does the call use up its closure argument before it returns?
    pub fn is_eager_callback(&self) -> bool {
        self.eager_callback
    }

    /// What releases the handle this call hands back, when it hands back a
    /// self-freeing one.
    pub fn fresh_release(&self) -> Option<&'static str> {
        self.fresh_release
    }
}

/// The shape of the stdlib symbol `qualified_name` (`Vec_push`, `fs_open`),
/// if a declaration is behind it.
pub fn declared_shape(qualified_name: &str) -> Option<CallShape> {
    lookup(qualified_name).map(CallShape::of_declaration)
}

/// What a call does with the argument at `arg_index`, by its declaration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArgMode {
    /// Read, or written through for a `mutate` parameter. The caller still
    /// owns it afterwards.
    Lent,
    /// Kept or freed: the caller has given it away.
    Given,
}

impl StdlibMethodMeta {
    /// How this declaration treats the argument at `arg_index`; MIR's
    /// argument zero is the receiver on a method.
    pub fn argument_mode(&self, arg_index: usize) -> ArgMode {
        if CallShape::of_declaration(self).gives(arg_index) { ArgMode::Given } else { ArgMode::Lent }
    }
}

// ── Return types ─────────────────────────────────────────────────

/// What a stub's declared return type is, in the categories MIR asks about.
fn ret_category(t: &TypeExpr) -> RetCategory {
    match t {
        TypeExpr::Unit | TypeExpr::NoReturn => RetCategory::Void,
        TypeExpr::Result { ok, err } => RetCategory::Result {
            ok: Box::new(ret_category(ok)),
            err: Box::new(ret_category(err)),
        },
        TypeExpr::Optional(inner) => RetCategory::Option(Box::new(ret_category(inner))),
        TypeExpr::RawPtr(_) => RetCategory::Ptr,
        TypeExpr::Tuple(parts) => RetCategory::Tuple(parts.iter().map(ret_category).collect()),
        TypeExpr::Named { args, .. } if t.name().as_deref() == Some("Option") && args.len() == 1 => {
            RetCategory::Option(Box::new(ret_category(&args[0])))
        }
        TypeExpr::Named { path, .. } => {
            let name = path.join(".");
            match name.as_str() {
                "bool" => RetCategory::Bool,
                "char" => RetCategory::Char,
                "string" => RetCategory::String,
                "i64" => RetCategory::I64,
                "i8" => RetCategory::Int(IntWidth::I8),
                "i16" => RetCategory::Int(IntWidth::I16),
                "i32" => RetCategory::Int(IntWidth::I32),
                "i128" => RetCategory::Int(IntWidth::I128),
                "u8" => RetCategory::Int(IntWidth::U8),
                "u16" => RetCategory::Int(IntWidth::U16),
                "u32" => RetCategory::Int(IntWidth::U32),
                "u64" => RetCategory::Int(IntWidth::U64),
                "u128" => RetCategory::Int(IntWidth::U128),
                "usize" => RetCategory::Int(IntWidth::Usize),
                "isize" => RetCategory::Int(IntWidth::Isize),
                "f32" | "f64" => RetCategory::F64,
                // A generic type variable like `T` is not a type — it's a hole.
                _ if t.bare_name().is_some_and(|n| {
                    n.len() == 1 && n.chars().all(|c| c.is_ascii_uppercase())
                }) =>
                {
                    RetCategory::TypeParam(name)
                }
                _ => RetCategory::Named(name),
            }
        }
        _ => RetCategory::Named(t.to_string()),
    }
}

/// Extract the type prefix from a return category.
fn ret_type_prefix(cat: &RetCategory) -> Option<std::string::String> {
    match cat {
        RetCategory::Void | RetCategory::Bool | RetCategory::I64 | RetCategory::Int(_)
        | RetCategory::F64 => None,
        // The stub doesn't know what `T` is, so it can't name a prefix for it.
        // Same answer it gave when `T` was recorded as I64.
        RetCategory::TypeParam(_) => None,
        RetCategory::String => Some("string".to_string()),
        // Keep the "char" prefix it had as a Named type, so `char` methods
        // still resolve on the result of a char-returning stdlib call.
        RetCategory::Char => Some("char".to_string()),
        RetCategory::Ptr => Some("Ptr".to_string()),
        RetCategory::Named(name) => Some(name.clone()),
        RetCategory::Tuple(_) => None,
        RetCategory::Option(_) => Some("Option".to_string()),
        RetCategory::Result { ok, .. } => {
            // The prefix is the ok type's prefix (e.g., Result<File, _> → "File")
            ret_type_prefix(ok)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(name: &str) -> TypeExpr {
        TypeExpr::named(name)
    }

    fn result(ok: TypeExpr, err: TypeExpr) -> TypeExpr {
        TypeExpr::Result { ok: Box::new(ok), err: Box::new(err) }
    }

    #[test]
    fn void() {
        assert_eq!(ret_category(&TypeExpr::Unit), RetCategory::Void);
    }

    #[test]
    fn primitives() {
        assert_eq!(ret_category(&n("bool")), RetCategory::Bool);
        assert_eq!(ret_category(&n("string")), RetCategory::String);
        assert_eq!(ret_category(&n("i64")), RetCategory::I64);
        assert_eq!(ret_category(&n("f64")), RetCategory::F64);
        // Every other width carries itself. They all used to collapse to `I64`,
        // so a `u64` that filled its range rendered as the signed reading of its
        // bits (#823).
        assert_eq!(ret_category(&n("usize")), RetCategory::Int(IntWidth::Usize));
        assert_eq!(ret_category(&n("u64")), RetCategory::Int(IntWidth::U64));
        assert_eq!(ret_category(&n("u8")), RetCategory::Int(IntWidth::U8));
        assert_eq!(ret_category(&n("i32")), RetCategory::Int(IntWidth::I32));
        assert_eq!(ret_category(&n("u128")), RetCategory::Int(IntWidth::U128));
    }

    #[test]
    fn results() {
        // The error side is read, not assumed. It used to be hardcoded I64,
        // which cost `T or JoinError` its enum identity all the way to codegen
        // (#677) and left every other error enum in the same shape.
        assert_eq!(
            ret_category(&result(n("File"), n("IoError"))),
            RetCategory::Result {
                ok: Box::new(RetCategory::Named("File".into())),
                err: Box::new(RetCategory::Named("IoError".into())),
            }
        );
        assert_eq!(
            ret_category(&result(TypeExpr::Unit, n("IoError"))),
            RetCategory::Result {
                ok: Box::new(RetCategory::Void),
                err: Box::new(RetCategory::Named("IoError".into())),
            }
        );
    }

    #[test]
    fn options() {
        assert_eq!(
            ret_category(&TypeExpr::Optional(Box::new(n("string")))),
            RetCategory::Option(Box::new(RetCategory::String))
        );
        assert_eq!(
            ret_category(&TypeExpr::generic("Option", vec![n("usize")])),
            RetCategory::Option(Box::new(RetCategory::Int(IntWidth::Usize)))
        );
    }

    #[test]
    fn named() {
        assert_eq!(ret_category(&n("File")), RetCategory::Named("File".into()));
        assert_eq!(
            ret_category(&TypeExpr::generic("Vec", vec![n("string")])),
            RetCategory::Named("Vec".into())
        );
    }

    #[test]
    fn pointers() {
        assert_eq!(ret_category(&TypeExpr::RawPtr(Box::new(n("u8")))), RetCategory::Ptr);
    }

    #[test]
    fn a_type_parameter_is_a_hole() {
        // A single-letter type variable is a hole, not a type. It used to be
        // recorded as I64, which is how a `Vec<string>.remove(i)` destination
        // got an 8-byte slot (#1020).
        assert_eq!(ret_category(&n("T")), RetCategory::TypeParam("T".into()));
        assert!(ret_category(&n("T")).names_a_type_param());
        assert!(ret_category(&TypeExpr::Optional(Box::new(n("T")))).names_a_type_param());
        assert!(ret_category(&result(n("T"), n("IoError"))).names_a_type_param());
        assert!(!ret_category(&n("string")).names_a_type_param());
        assert!(!ret_category(&n("File")).names_a_type_param());
    }

    #[test]
    fn result_prefix_is_ok_type() {
        let cat = ret_category(&result(n("File"), n("IoError")));
        assert_eq!(ret_type_prefix(&cat), Some("File".into()));
    }

    #[test]
    fn void_result_has_no_prefix() {
        let cat = ret_category(&result(TypeExpr::Unit, n("IoError")));
        assert_eq!(ret_type_prefix(&cat), None);
    }

    #[test]
    fn cache_has_types_and_modules() {
        let types = stdlib_type_names();
        let mods = stdlib_module_names();
        assert!(types.contains("Vec"), "missing Vec type");
        assert!(types.contains("string"), "missing string type");
        assert!(mods.contains("fs"), "missing fs module");
        assert!(mods.contains("json"), "missing json module");
    }

    #[test]
    fn cache_has_method_metas() {
        let metas = method_metas();
        assert!(!metas.is_empty(), "no method metas");
        // Spot-check a known method
        let vec_push = lookup("Vec_push");
        assert!(vec_push.is_some(), "missing Vec_push meta");
        assert_eq!(vec_push.unwrap().ret_category, RetCategory::Void);
    }

    #[test]
    fn tcp_listener_accept_returns_result() {
        let meta = lookup("TcpListener_accept").expect("missing TcpListener_accept");
        assert!(matches!(meta.ret_category, RetCategory::Result { .. }),
            "expected Result, got {:?}", meta.ret_category);
    }

    #[test]
    fn fs_open_returns_result_with_file_prefix() {
        let meta = lookup("fs_open").expect("missing fs_open");
        assert!(matches!(meta.ret_category, RetCategory::Result { .. }));
        assert_eq!(meta.ret_type_prefix, Some("File".into()));
    }

    #[test]
    fn string_from_raw_returns_string() {
        let meta = lookup("string_from_raw").expect("missing string_from_raw");
        assert_eq!(meta.ret_category, RetCategory::String);
    }

    /// Names a stub signature may mention that `stdlib_type_names` doesn't hold.
    ///
    /// Two groups. Declared elsewhere and genuinely resolvable: `Ordering` is
    /// registered by the checker rather than by a stub;
    /// `Reader`/`Writer` are stdlib interfaces in io.rk, and the stub registry only
    /// collects structs and enums; `Self` isn't a type name at all. Genuinely still missing: `Error` is the
    /// `any Error` catch-all that ER32 auto-boxing will register (#708).
    const PENDING_STUB_TYPES: &[&str] = &[
        "Ordering", "Reader", "Writer", "Self", "Error",
    ];

    /// PascalCase type names a signature string mentions, with generic
    /// arguments, `T or E` branches and pointer/optional markers peeled off.
    /// Single letters are type parameters, not types.
    fn named_types_in(ty: &TypeExpr) -> Vec<std::string::String> {
        let mut out = Vec::new();
        ty.walk_paths(&mut |path| {
            for w in path {
                if w.len() > 1 && w.starts_with(|c: char| c.is_ascii_uppercase()) {
                    out.push(w.clone());
                }
            }
        });
        out
    }

    /// Every named type a stub signature mentions has to be one that exists.
    ///
    /// `Vec.try_push` was declared `-> void or PushError<T>` for as long as the
    /// stub existed, and `PushError` was never declared anywhere: the error had
    /// no variants, no constructor and no `message()`, so the rejected value it
    /// was meant to hand back could not be read (#666). Nothing caught it
    /// because stub signatures aren't type-checked — a name that resolves to
    /// nothing just stays unresolved until some unlucky call site trips on it.
    #[test]
    fn stub_signatures_only_name_types_that_exist() {
        let reg = StubRegistry::load();
        let known = stdlib_type_names();
        let mut missing: Vec<std::string::String> = Vec::new();

        let mut check = |ty: &TypeExpr, at: std::string::String, missing: &mut Vec<std::string::String>| {
            for name in named_types_in(ty) {
                if !known.contains(&name) && !PENDING_STUB_TYPES.contains(&name.as_str()) {
                    missing.push(format!("{} names unknown type `{}`", at, name));
                }
            }
        };

        for type_name in reg.type_names() {
            for m in reg.methods(type_name) {
                check(&m.ret_ty, format!("{}.{} return", type_name, m.name), &mut missing);
                for (pname, pty) in &m.params {
                    check(pty, format!("{}.{} param `{}`", type_name, m.name, pname), &mut missing);
                }
            }
        }
        for f in reg.functions() {
            check(&f.ret_ty, format!("{} return", f.name), &mut missing);
        }

        missing.sort();
        assert!(
            missing.is_empty(),
            "stub signatures name types that don't exist:\n  {}",
            missing.join("\n  ")
        );
    }

    fn shape(name: &str) -> CallShape {
        declared_shape(name).unwrap_or_else(|| panic!("{name} is not declared"))
    }

    /// The ownership questions, answered from the declarations.
    ///
    /// These replaced prefix lists kept by hand in `rc_elide`, `rc_insert` and
    /// `container_drop` — lists that had drifted apart from each other and from
    /// the stdlib. Pinning the answers here means a stdlib signature can't
    /// quietly change what MIR believes about a call: getting one wrong is a
    /// leak in one direction and a double free in the other.
    #[test]
    fn ownership_questions_follow_the_declarations() {
        // Stored: the reference moves in with the value.
        assert!(shape("Vec_push").gives(1), "Vec.push keeps its value");
        assert!(shape("Map_insert").gives(1), "Map.insert keeps its key");
        assert!(shape("Map_insert").gives(2), "Map.insert keeps its value");
        assert!(shape("Vec_insert").gives(2), "Vec.insert keeps its value");
        assert!(shape("Shared_new").gives(0), "Shared.new keeps its payload");
        assert!(shape("Shared_local").gives(0), "Shared.local keeps its payload");

        // Read and forgotten: the caller still owns what it passed.
        assert!(!shape("Map_get").gives(1), "Map.get only reads the key");
        assert!(!shape("Map_contains").gives(1));
        assert!(!shape("Vec_insert").gives(1), "the index is a number");
        // The receiver is never "kept" by a borrowing method.
        assert!(!shape("Vec_push").gives(0));

        // A view into the receiver's own storage: releasing it here would free
        // what the container still holds.
        assert!(shape("Vec_get").returns_a_view());
        assert!(shape("Map_get").returns_a_view());
        assert!(!shape("Vec_len").returns_a_view(), "a length is not a view");
        assert!(!shape("Vec_new").returns_a_view(), "a constructor has no receiver");
        assert!(!shape("Vec_remove").returns_a_view(), "`@moves_out` hands the element over");

        // Borrowed receivers, so the drop pass may still free the container.
        assert_eq!(shape("Vec_push").receiver(), Receiver::Borrowed);
        assert_eq!(shape("Vec_get").receiver(), Receiver::Borrowed);
        assert_eq!(shape("Vec_new").receiver(), Receiver::Absent, "static: no receiver at all");
        assert_eq!(shape("Handle_join").receiver(), Receiver::Consumed, "declared `take self`");
        assert!(!shape("Handle_join").returns_a_view(), "`take self` leaves nothing to point into");

        assert!(shape("Vec_remove").moves_out());
        assert!(shape("Vec_sort_by").is_eager_callback());
        assert!(!shape("Vec_map").is_eager_callback(), "a lazy adapter keeps its closure");
    }

    /// A `Random` is a handle that frees itself, so a call that makes one hands
    /// back something the caller releases.
    #[test]
    fn a_fresh_handle_names_its_release() {
        assert!(shape("Random_new").fresh_release().is_some());
        assert!(shape("Vec_new").fresh_release().is_none(), "a container's free also walks its elements");
    }

    /// A program function named like a stdlib symbol, or like one lowering
    /// makes up for a stdlib type, gets a symbol of its own (#1307).
    #[test]
    fn the_stdlib_owns_its_symbol_namespace() {
        assert!(is_method_symbol("Vec_len"));
        assert!(is_method_symbol("Vec_index"), "`v[i]` is a symbol no declaration lists");
        assert!(is_method_symbol("Cell_acquire"));
        assert!(is_method_symbol("i64_to_string"));
        assert!(!is_method_symbol("helper"));
        assert!(!is_method_symbol("render_table"));
    }
}

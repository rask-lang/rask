// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Interface checking for Rask.
//!
//! Implements structural interface satisfaction: a type satisfies an interface if it has
//! all required methods with matching signatures.

use crate::types::{GenericArg, Type, TypeId};
use crate::checker::{TypeTable, TypeDef, MethodSig, SelfParam, ParamMode};
use rask_ast::Span;
use std::collections::HashMap;
use thiserror::Error;

// ============================================================================
// Interface Bound
// ============================================================================

/// An interface bound like `T: Comparable` or `K: Hashable + Clone`.
#[derive(Debug, Clone)]
pub struct InterfaceBound {
    /// The type parameter name (e.g., "T").
    pub type_param: String,
    /// The interfaces it must satisfy.
    pub interfaces: Vec<String>,
}

impl InterfaceBound {
    pub fn new(type_param: impl Into<String>, interfaces: Vec<String>) -> Self {
        Self {
            type_param: type_param.into(),
            interfaces,
        }
    }

    pub fn single(type_param: impl Into<String>, interface_name: impl Into<String>) -> Self {
        Self {
            type_param: type_param.into(),
            interfaces: vec![interface_name.into()],
        }
    }
}

// ============================================================================
// Interface Errors
// ============================================================================

/// Errors during interface checking.
#[derive(Debug, Error)]
pub enum InterfaceError {
    #[error("Type {ty} does not satisfy interface {interface_name}")]
    NotSatisfied { ty: String, interface_name: String, span: Span },

    #[error("Missing method '{method}' required by interface {interface_name}")]
    MissingMethod {
        ty: String,
        interface_name: String,
        method: String,
        /// The signature the interface asks for, so the message can show the line
        /// to write rather than only the name that's absent.
        signature: String,
        span: Span,
    },

    #[error("Method '{method}' signature mismatch: expected {expected}, found {found}")]
    SignatureMismatch {
        ty: String,
        method: String,
        expected: String,
        found: String,
        span: Span,
    },

    #[error("Unknown interface: {0}")]
    UnknownInterface(String),

    #[error("Conflicting method signatures in composed interfaces: {method}")]
    ConflictingMethods { method: String, interface1: String, interface2: String },
}

// ============================================================================
// Interface Checker
// ============================================================================

/// Checks structural interface satisfaction.
pub struct InterfaceChecker<'a> {
    /// The type table containing all type definitions.
    types: &'a TypeTable,
    /// Collected errors.
    errors: Vec<InterfaceError>,
    /// Cache for interface method requirements (expanded with composed interfaces).
    interface_methods: HashMap<String, Vec<MethodSig>>,
}

impl<'a> InterfaceChecker<'a> {
    pub fn new(types: &'a TypeTable) -> Self {
        let mut checker = Self {
            types,
            errors: Vec::new(),
            interface_methods: HashMap::new(),
        };
        checker.collect_interface_methods();
        checker
    }

    /// Collect all methods from interfaces (including composed interfaces).
    fn collect_interface_methods(&mut self) {
        // First pass: collect direct methods
        let mut super_map: Vec<(String, Vec<String>)> = Vec::new();
        for def in self.types.iter() {
            if let TypeDef::Interface { name, super_interfaces, methods, .. } = def {
                self.interface_methods.insert(name.clone(), methods.clone());
                if !super_interfaces.is_empty() {
                    super_map.push((name.clone(), super_interfaces.clone()));
                }
            }
        }
        // Second pass: add inherited methods from super-interfaces
        for (interface_name, supers) in &super_map {
            let mut inherited = Vec::new();
            for parent in supers {
                if let Some(parent_methods) = self.interface_methods.get(parent) {
                    for m in parent_methods {
                        // Don't duplicate methods already defined directly
                        if !self.interface_methods.get(interface_name)
                            .map_or(false, |ms| ms.iter().any(|existing| existing.name == m.name))
                            && !inherited.iter().any(|im: &MethodSig| im.name == m.name)
                        {
                            inherited.push(m.clone());
                        }
                    }
                }
            }
            if let Some(methods) = self.interface_methods.get_mut(interface_name) {
                methods.extend(inherited);
            }
        }
    }

    /// G1: is this a nominal user-declared interface (registered, not `duck`)?
    /// Builtin/auto-derived interfaces (Equal, Comparable, …) are handled by
    /// eligibility and keep structural matching; only user-declared interfaces
    /// require an explicit `extend T implements Interface` conformance.
    fn is_nominal_user_interface(&self, interface_name: &str) -> bool {
        let base = interface_name.split('<').next().unwrap_or(interface_name);
        // A compiler-provided interface is satisfied by shape, whether or not
        // `stdlib/` also writes the declaration down. `Displayable` means "has
        // `to_string`" — std.fmt/D5 says an error type gets it from `message()`
        // alone — and `Error`, `Debug` and `Hashable` are the same kind of rule.
        //
        // The G1 gate below is for an interface a program *declares*, where a
        // matching shape without `extend T implements Interface` is deliberately rejected.
        // Reading the name off a declaration alone conflated the two: putting
        // `fmt.rk` in the stub set gave `Displayable` a declaration and every
        // inherent `to_string` in the stdlib stopped counting — `StringView`
        // declares one and was reported as not implementing the interface (#990).
        if COMPILER_PROVIDED_TRAITS.contains(&base) {
            return false;
        }
        matches!(
            self.types.get_type_id(base).and_then(|id| self.types.get(id)),
            Some(TypeDef::Interface { is_duck: false, .. })
        )
    }

    /// The registered TypeId of a struct/enum type, for conformance lookup.
    fn user_type_id(&self, ty: &Type) -> Option<crate::types::TypeId> {
        let id = match ty {
            Type::Named(id) => *id,
            Type::Generic { base, .. } => *base,
            Type::UnresolvedNamed(name) => self.types.get_type_id(name)?,
            Type::UnresolvedGeneric { name, .. } => self.types.get_type_id(name)?,
            _ => return None,
        };
        matches!(self.types.get(id), Some(TypeDef::Struct { .. } | TypeDef::Enum { .. }))
            .then_some(id)
    }

    /// The registered TypeId behind a type name, whatever kind it is.
    ///
    /// `user_type_id` narrows to structs and enums, which is what conformance
    /// lookup wants. A nominal newtype has to be reachable too — its `implements`
    /// clause is a conformance declaration of a different shape.
    fn named_type_id(&self, ty: &Type) -> Option<crate::types::TypeId> {
        match ty {
            Type::Named(id) => Some(*id),
            Type::Generic { base, .. } => Some(*base),
            Type::UnresolvedNamed(name) => self.types.get_type_id(name),
            Type::UnresolvedGeneric { name, .. } => self.types.get_type_id(name),
            _ => None,
        }
    }

    /// Check if a type satisfies an interface bound.
    pub fn check_satisfies(
        &mut self,
        ty: &Type,
        interface_name: &str,
        span: Span,
    ) -> Result<(), InterfaceError> {
        // Encode/Decode are structural markers (std.encoding E12–E17): a type
        // satisfies them by shape, not by a declared `extend`. A base type, a
        // container of encodable elements, or a struct/enum whose fields all
        // encode qualifies. These aren't registered as interfaces, so short-circuit
        // before the method-based logic (which would fail with UnknownInterface).
        let base_interface = interface_name.split('<').next().unwrap_or(interface_name);

        // NT1–NT3: every primitive of the right kind satisfies `Numeric`,
        // `Integer` and `Float` — that's what the rules say those names mean,
        // and the widths and constants they promise (`MIN`, `MAX`, `BITS`,
        // `EPSILON`) aren't things the method-based check below can see.
        //
        // A non-primitive falls through rather than being rejected here.
        // `Numeric` is a nominal interface in the roster with eight methods, and
        // OP1 says generic operator use goes through it "like any other
        // generic call" — so a type that declares the conformance has to count
        // too. Short-circuiting to a membership test alone would have made
        // `extend MyDecimal implements Numeric` unusable as a bound.
        //
        // Unregistered, these names failed at every call site: `func
        // narrow<T: Integer>` reported "`_` does not implement `Integer`" —
        // `_` because an unknown interface has no type to blame, and unknown
        // because nothing had ever registered the name (#713).
        if let Some(members) = numeric_interface_members(base_interface) {
            if members(ty) {
                return Ok(());
            }
        }

        if matches!(base_interface, "Encode" | "Decode") {
            // E16: the owner's opt-out comes first. Its fields may all be
            // perfectly serializable — that's usually why the annotation is
            // there, on a credential or a raw handle whose bytes would go
            // somewhere they shouldn't.
            if self.opts_out_of(ty, base_interface) {
                return Err(InterfaceError::NotSatisfied {
                    ty: self.type_name(ty),
                    interface_name: interface_name.to_string(),
                    span,
                });
            }
            let structurally_ok = self.type_is_encodable(ty, &mut Vec::new());
            // E13a: decoding has one requirement encoding doesn't. Encoding never
            // needs a value for a field it leaves out; decoding has to build the
            // whole struct, so an excluded field with no default has nothing to
            // come from.
            let decodable = base_interface != "Decode"
                || self.first_defaultless_excluded_field(ty).is_none();
            if structurally_ok && decodable {
                return Ok(());
            }
            return Err(InterfaceError::NotSatisfied {
                ty: self.type_name(ty),
                interface_name: interface_name.to_string(),
                span,
            });
        }

        // TU8/TU9/TU11: a tuple's value semantics are element-wise, the same way
        // a struct's are field-wise (TU10 makes the layouts the same thing). Its
        // elements have no names to hang an `extend` block on, so a tuple can only
        // ever get a conformance this way — and it had none, so `Map<(i64, i64),
        // V>` failed the moment the Map key bound became a real check (#812).
        //
        // A fixed array is the same argument with one element type.
        if matches!(base_interface, "Equal" | "Hashable" | "Cloneable") {
            let elems: Option<Vec<Type>> = match ty {
                Type::Tuple(elems) => Some(elems.clone()),
                Type::Array { elem, .. } => Some(vec![(**elem).clone()]),
                _ => None,
            };
            if let Some(elems) = elems {
                if elems.iter().all(|e| {
                    self.check_satisfies(e, base_interface, span).is_ok()
                }) {
                    return Ok(());
                }
                return Err(InterfaceError::NotSatisfied {
                    ty: self.type_name(ty),
                    interface_name: interface_name.to_string(),
                    span,
                });
            }
        }

        // T11: a nominal newtype inherits exactly the interfaces its `implements`
        // clause lists, delegating to the value it wraps. Method resolution
        // already honoured that, so `UserId(1) == UserId(2)` worked — but this
        // check didn't, so `implements_interface(UserId, "Hashable")` said no for a
        // type whose declaration says yes. Nothing asked until the Map key bound
        // became a real conformance check (#812).
        //
        // Not listed falls through rather than being rejected here: `Debug`
        // applies to every type and doesn't come from the clause.
        if let Some(TypeDef::NominalAlias { with_interfaces, .. }) =
            self.named_type_id(ty).and_then(|id| self.types.get(id))
        {
            if with_interfaces.iter().any(|t| {
                t.split('<').next().unwrap_or(t).trim() == base_interface
            }) {
                return Ok(());
            }
        }

        // G1 nominal gate: a user struct/enum satisfies a user-declared interface
        // only through a declared `extend T implements Interface` (or auto-derive). A
        // matching shape without the declaration is rejected — the flip.
        if self.is_nominal_user_interface(interface_name) {
            if let Some(type_id) = self.user_type_id(ty) {
                if !self.types.declares_conformance(type_id, interface_name) {
                    return Err(InterfaceError::NotSatisfied {
                        ty: self.type_name(ty),
                        interface_name: interface_name.to_string(),
                        span,
                    });
                }
                // CC1: a conditional conformance holds only for instantiations
                // that satisfy the `where` clause, checked here per instantiation.
                if let Some(cond) = self.types.conformance_condition(type_id, interface_name).cloned() {
                    if let Some(err) = self.check_conformance_condition(ty, type_id, &cond, span) {
                        return Err(err);
                    }
                }
            }
        }

        // GT2/AT6: the interface's parameters come from the header, and each
        // `Self.X` from what the conformance answers with. Without this the
        // signature still says `Rhs` and `Self.Out`, neither of which any
        // implementation can match — which is what made every conformance to a
        // generic interface fail claiming a missing method (#1164).
        let subst = self.conformance_substitution(ty, interface_name);
        let required_methods: Vec<MethodSig> = self
            .get_interface_methods(interface_name)?
            .into_iter()
            .map(|m| substitute_signature(&m, &subst))
            .collect();

        // Get the type's available methods
        let type_methods = self.get_type_methods(ty);

        // Check each required method exists with matching signature
        for required in &required_methods {
            // GT3/AT8: a type may carry two conformances of one generic interface
            // (`Mul<f64>` and `Mul<Meters>`), and then two methods answer to
            // one name. Match the one this conformance asks for; falling back
            // to the first by name checked `Mul<Meters>` against the `f64`
            // method and reported a mismatch on a block that was correct.
            // OR4: an operator conformance files its method under the applied
            // argument (`mul$f64`), so compare on the name the interface asked for.
            let by_name: Vec<&MethodSig> = type_methods
                .iter()
                .filter(|m| rask_ast::operators::method_display(&m.name) == required.name)
                .collect();
            let found = by_name
                .iter()
                .find(|m| self.signatures_match(required, m))
                .or(by_name.first())
                .copied();
            if let Some(found) = found {
                // Check signature matches
                if !self.signatures_match(required, found) {
                    return Err(InterfaceError::SignatureMismatch {
                        ty: self.type_name(ty),
                        method: required.name.clone(),
                        expected: self.format_signature(required),
                        found: self.format_signature(found),
                        span,
                    });
                }
            } else {
                // Check for primitive/builtin methods
                if !self.has_builtin_method(ty, &required.name) {
                    return Err(InterfaceError::MissingMethod {
                        ty: self.type_name(ty),
                        interface_name: interface_name.to_string(),
                        method: required.name.clone(),
                        signature: self.format_signature(required),
                        span,
                    });
                }
            }
        }

        Ok(())
    }

    /// std.encoding E12–E17: does `ty` encode structurally? Base types, optionals,
    /// tuples/arrays, the `Vec`/`Map`/`Set` containers, and structs/enums whose
    /// public fields (variant payloads) all encode. `visited` breaks cycles in
    /// recursive types — a self-referential field is treated coinductively.
    /// E16: does this type's declaration refuse `Encode`/`Decode` outright?
    ///
    /// Only the named type itself. A `Vec<Secret>` is already not encodable
    /// because `Secret` isn't, and reporting the container would name the
    /// wrong declaration.
    pub fn opts_out_of(&self, ty: &Type, base_interface: &str) -> bool {
        let (Type::Named(id) | Type::Generic { base: id, .. }) = ty else {
            return false;
        };
        let (no_encode, no_decode) = match self.types.get(*id) {
            Some(TypeDef::Struct { no_encode, no_decode, .. })
            | Some(TypeDef::Enum { no_encode, no_decode, .. }) => (*no_encode, *no_decode),
            _ => return false,
        };
        match base_interface {
            "Encode" => no_encode,
            "Decode" => no_decode,
            _ => false,
        }
    }

    fn type_is_encodable(&self, ty: &Type, visited: &mut Vec<TypeId>) -> bool {
        use crate::types::GenericArg;
        match ty {
            // E14: base types
            Type::Bool
            | Type::Char
            | Type::String
            | Type::Unit
            | Type::I8
            | Type::I16
            | Type::I32
            | Type::I64
            | Type::I128
            | Type::U8
            | Type::U16
            | Type::U32
            | Type::U64
            | Type::U128
            | Type::F32
            | Type::F64 => true,
            // E15: `T?` (Result with an absent err) encodes when its payload does
            Type::Result { ok, err } if matches!(**err, Type::None) => {
                self.type_is_encodable(ok, visited)
            }
            Type::Array { elem, .. } => self.type_is_encodable(elem, visited),
            Type::Tuple(elems) => elems.iter().all(|e| self.type_is_encodable(e, visited)),
            Type::Named(id) => self.named_is_encodable(*id, &[], visited),
            Type::UnresolvedNamed(name) => match self.types.get_type_id(name) {
                Some(id) => self.named_is_encodable(id, &[], visited),
                None => false,
            },
            Type::Generic { base, args } => {
                let targs: Vec<Type> = args
                    .iter()
                    .filter_map(|a| match a {
                        GenericArg::Type(t) => Some((**t).clone()),
                        _ => None,
                    })
                    .collect();
                let name = self.types.get(*base).map(Self::type_def_name);
                self.container_or_named_encodable(name.as_deref(), Some(*base), &targs, visited)
            }
            Type::UnresolvedGeneric { name, args } => {
                let targs: Vec<Type> = args
                    .iter()
                    .filter_map(|a| match a {
                        GenericArg::Type(t) => Some((**t).clone()),
                        _ => None,
                    })
                    .collect();
                let id = self.types.get_type_id(name);
                self.container_or_named_encodable(Some(name), id, &targs, visited)
            }
            _ => false,
        }
    }

    /// The first field that keeps a type from being Encode/Decode, as
    /// `("home.zip", "Socket")`. Only for the diagnostic — "this struct isn't
    /// serializable" without saying which field leaves you reading the
    /// declaration line by line (the shape std.encoding's E12 error shows).
    ///
    /// Returns None for a type that qualifies, or one that fails for a reason
    /// other than a field (a bare pointer asked about directly, say).
    pub fn first_unencodable_field(&self, ty: &Type) -> Option<(String, String)> {
        let id = match ty {
            Type::Named(id) => Some(*id),
            Type::UnresolvedNamed(name) => self.types.get_type_id(name),
            Type::Generic { base, .. } => Some(*base),
            Type::UnresolvedGeneric { name, .. } => self.types.get_type_id(name),
            _ => None,
        }?;
        self.unencodable_field_of(id, &mut Vec::new())
    }

    /// E13a: the first field the wire form leaves out that has no default to
    /// fill it from on decode. Reported by name, since the fix is on that field.
    pub fn first_defaultless_excluded_field(&self, ty: &Type) -> Option<String> {
        let id = match ty {
            Type::Named(id) => Some(*id),
            Type::UnresolvedNamed(name) => self.types.get_type_id(name),
            Type::Generic { base, .. } => Some(*base),
            Type::UnresolvedGeneric { name, .. } => self.types.get_type_id(name),
            _ => None,
        }?;
        self.defaultless_excluded_field_of(id, &mut Vec::new())
    }

    fn defaultless_excluded_field_of(
        &self,
        id: TypeId,
        visited: &mut Vec<TypeId>,
    ) -> Option<String> {
        if visited.contains(&id) {
            return None;
        }
        visited.push(id);
        let Some(TypeDef::Struct { fields, undecodable_fields, .. }) = self.types.get(id) else {
            return None;
        };
        if let Some(name) = undecodable_fields.first() {
            return Some(name.clone());
        }
        // A nested struct's own excluded fields block the outer type too — the
        // decode builds it the same way.
        fields.iter().find_map(|(fname, fty)| {
            let nested = match fty {
                Type::Named(nid) => Some(*nid),
                Type::UnresolvedNamed(n) => self.types.get_type_id(n),
                _ => None,
            }?;
            self.defaultless_excluded_field_of(nested, &mut visited.clone())
                .map(|inner| format!("{}.{}", fname, inner))
        })
    }

    fn unencodable_field_of(
        &self,
        id: TypeId,
        visited: &mut Vec<TypeId>,
    ) -> Option<(String, String)> {
        if visited.contains(&id) {
            return None;
        }
        visited.push(id);
        let found = match self.types.get(id) {
            Some(TypeDef::Struct { fields, private_fields, skipped_fields, .. }) => {
                fields.iter().find_map(|(fname, fty)| {
                    if private_fields.contains(fname) || skipped_fields.contains(fname) {
                        return None;
                    }
                    if self.type_is_encodable(fty, &mut visited.clone()) {
                        return None;
                    }
                    // Point at the innermost field, so a nested struct reports
                    // `home.zip` rather than just `home`.
                    match self.nested_field_id(fty) {
                        Some(inner) => match self.unencodable_field_of(inner, visited) {
                            Some((path, ty_name)) => Some((format!("{}.{}", fname, path), ty_name)),
                            None => Some((fname.clone(), self.display_ty(fty))),
                        },
                        None => Some((fname.clone(), self.display_ty(fty))),
                    }
                })
            }
            _ => None,
        };
        visited.pop();
        found
    }

    /// How a field's type reads in a message — `*u8`, not `RawPtr(U8)`.
    fn display_ty(&self, ty: &Type) -> String {
        format!("{}", self.types.resolve_type_names(ty))
    }

    fn nested_field_id(&self, ty: &Type) -> Option<TypeId> {
        match ty {
            Type::Named(id) => Some(*id),
            Type::UnresolvedNamed(name) => self.types.get_type_id(name),
            _ => None,
        }
    }

    /// A container spelling (`Vec`/`Map`/`Set`) encodes when its element types do;
    /// any other generic is a user struct/enum instantiation, checked field-wise.
    fn container_or_named_encodable(
        &self,
        name: Option<&str>,
        id: Option<TypeId>,
        targs: &[Type],
        visited: &mut Vec<TypeId>,
    ) -> bool {
        if matches!(name, Some("Vec" | "Map" | "Set")) {
            return targs.iter().all(|t| self.type_is_encodable(t, visited));
        }
        match id {
            Some(id) => self.named_is_encodable(id, targs, visited),
            None => false,
        }
    }

    fn type_def_name(def: &TypeDef) -> &str {
        match def {
            TypeDef::Struct { name, .. }
            | TypeDef::Enum { name, .. }
            | TypeDef::Interface { name, .. }
            | TypeDef::Union { name, .. }
            | TypeDef::NominalAlias { name, .. }
            | TypeDef::Primitive { name, .. } => name,
        }
    }

    /// Encodability of a named struct/enum, with `targs` bound to its type params.
    fn named_is_encodable(&self, id: TypeId, targs: &[Type], visited: &mut Vec<TypeId>) -> bool {
        if visited.contains(&id) {
            return true; // recursive type — assume ok, the non-cyclic fields decide
        }
        visited.push(id);
        let result = match self.types.get(id) {
            Some(TypeDef::Struct { fields, type_params, private_fields, skipped_fields, .. }) => {
                let subst = Self::build_subst(type_params, targs);
                // E12: only public fields participate, and E19 takes `@skip`
                // fields out of the wire form entirely — a skipped field holds
                // whatever it likes without blocking the type.
                fields.iter().all(|(fname, fty)| {
                    private_fields.contains(fname)
                        || skipped_fields.contains(fname)
                        || self.type_is_encodable(&Self::apply_subst(fty, &subst), visited)
                })
            }
            Some(TypeDef::Enum { variants, type_params, .. }) => {
                let subst = Self::build_subst(type_params, targs);
                variants.iter().all(|(_, payloads)| {
                    payloads
                        .iter()
                        .all(|pty| self.type_is_encodable(&Self::apply_subst(pty, &subst), visited))
                })
            }
            Some(TypeDef::NominalAlias { underlying, .. }) => {
                self.type_is_encodable(&underlying.clone(), visited)
            }
            _ => false,
        };
        visited.pop();
        result
    }

    fn build_subst(type_params: &[String], targs: &[Type]) -> HashMap<String, Type> {
        type_params
            .iter()
            .cloned()
            .zip(targs.iter().cloned())
            .collect()
    }

    /// Replace bare type-parameter references in `ty` with their bound arguments.
    /// Only substitutes at the positions that matter for encodability (the field's
    /// own type and container element args); anything else passes through.
    fn apply_subst(ty: &Type, subst: &HashMap<String, Type>) -> Type {
        use crate::types::GenericArg;
        match ty {
            Type::UnresolvedNamed(name) => subst.get(name).cloned().unwrap_or_else(|| ty.clone()),
            Type::Generic { base, args } => Type::Generic {
                base: *base,
                args: args
                    .iter()
                    .map(|a| match a {
                        GenericArg::Type(t) => GenericArg::Type(Box::new(Self::apply_subst(t, subst))),
                        other => other.clone(),
                    })
                    .collect(),
            },
            Type::UnresolvedGeneric { name, args } => Type::UnresolvedGeneric {
                name: name.clone(),
                args: args
                    .iter()
                    .map(|a| match a {
                        GenericArg::Type(t) => GenericArg::Type(Box::new(Self::apply_subst(t, subst))),
                        other => other.clone(),
                    })
                    .collect(),
            },
            Type::Result { ok, err } => Type::Result {
                ok: Box::new(Self::apply_subst(ok, subst)),
                err: Box::new(Self::apply_subst(err, subst)),
            },
            Type::Array { elem, len } => Type::Array {
                elem: Box::new(Self::apply_subst(elem, subst)),
                len: *len,
            },
            Type::Tuple(elems) => {
                Type::Tuple(elems.iter().map(|e| Self::apply_subst(e, subst)).collect())
            }
            other => other.clone(),
        }
    }

    /// CC1: verify a conditional conformance's `where` clause against the
    /// concrete generic arguments. Maps the type's params to the instantiation's
    /// args and checks each bound. Returns the first failure, or None if the
    /// condition holds (or the args aren't concrete yet — deferred).
    fn check_conformance_condition(
        &mut self,
        ty: &Type,
        type_id: crate::types::TypeId,
        cond: &[(String, Vec<String>)],
        span: Span,
    ) -> Option<InterfaceError> {
        use crate::types::GenericArg;
        let type_params = match self.types.get(type_id) {
            Some(TypeDef::Struct { type_params, .. } | TypeDef::Enum { type_params, .. }) => {
                type_params.clone()
            }
            _ => return None,
        };
        let args: Vec<Type> = match ty {
            Type::Generic { args, .. } => args.iter().filter_map(|a| match a {
                GenericArg::Type(t) => Some((**t).clone()),
                _ => None,
            }).collect(),
            // Not instantiated with concrete type args — defer (checked at the
            // outermost concrete use).
            _ => return None,
        };
        // Bail if any argument is still abstract (a type var or bare param) —
        // the condition is verified once the args become concrete.
        if args.iter().any(is_abstract_arg) {
            return None;
        }
        let subst: std::collections::HashMap<&str, &Type> =
            type_params.iter().map(|s| s.as_str()).zip(args.iter().map(|t| t)).collect();
        for (param, bounds) in cond {
            if let Some(arg_ty) = subst.get(param.as_str()) {
                let arg_ty = (*arg_ty).clone();
                for bound in bounds {
                    if let Err(e) = self.check_satisfies(&arg_ty, bound, span) {
                        return Some(e);
                    }
                }
            }
        }
        None
    }

    /// Check if a type satisfies all bounds.
    pub fn check_bounds(
        &mut self,
        concrete_type: &Type,
        bounds: &[InterfaceBound],
        span: Span,
    ) -> Vec<InterfaceError> {
        let mut errors = Vec::new();

        for bound in bounds {
            for interface_name in &bound.interfaces {
                if let Err(e) = self.check_satisfies(concrete_type, interface_name, span) {
                    errors.push(e);
                }
            }
        }

        errors
    }

    /// Get methods required by an interface (public accessor for interface object resolution).
    pub fn get_interface_methods_public(&self, interface_name: &str) -> Vec<MethodSig> {
        self.get_interface_methods(interface_name).unwrap_or_default()
    }

    /// GT2/AT6: what an interface's written signatures mean for one conformance.
    ///
    /// Maps `Rhs` to the argument the header gave it (or the declared default),
    /// and `Self.Out` to what the conformance's `type Out = ...` named (or the
    /// interface's default for it). Both are lookups: nothing is solved for.
    pub fn conformance_substitution(
        &self,
        self_ty: &Type,
        interface_ref: &str,
    ) -> HashMap<String, Type> {
        let mut map = HashMap::new();
        let base = interface_ref.split('<').next().unwrap_or(interface_ref).trim();
        let Some(TypeDef::Interface { type_params, assoc_types, .. }) =
            self.types.get_type_id(base).and_then(|id| self.types.get(id))
        else {
            return map;
        };

        let written = crate::checker::type_table::interface_ref_args(interface_ref);
        for (i, p) in type_params.iter().enumerate() {
            let arg = written.get(i).cloned().or_else(|| p.default.clone());
            if let Some(arg) = arg {
                if arg == "Self" {
                    map.insert(p.name.clone(), self_ty.clone());
                } else if let Ok(t) = crate::checker::parse_type_string(&arg, self.types) {
                    map.insert(p.name.clone(), t);
                }
            }
        }

        if assoc_types.is_empty() {
            return map;
        }
        let type_id = self.named_type_id(self_ty);
        for a in assoc_types {
            let bound = type_id
                .and_then(|id| self.types.assoc_binding(id, interface_ref, &a.name))
                .cloned()
                .or_else(|| match a.default.as_deref() {
                    Some("Self") => Some(self_ty.clone()),
                    Some(d) => crate::checker::parse_type_string(d, self.types).ok(),
                    None => None,
                });
            if let Some(t) = bound {
                map.insert(format!("Self.{}", a.name), t);
            } else {
                // AT2: the conformance never said. Leave the projection as
                // written so the unanswered-associated-type error is the one
                // the author sees, not a pile of signature mismatches.
            }
        }
        map
    }

    /// MN3: what a conformance of `interface_ref` by `self_ty` has to provide, with
    /// the interface's parameters and associated types already filled in.
    pub fn required_signatures(&self, self_ty: &Type, interface_ref: &str) -> Vec<MethodSig> {
        let subst = self.conformance_substitution(self_ty, interface_ref);
        self.get_interface_methods(interface_ref)
            .unwrap_or_default()
            .into_iter()
            .map(|m| substitute_signature(&m, &subst))
            .collect()
    }

    /// CD2: every method name the interface declares, its parents' included.
    /// `None` when the interface is unknown, which is its own error.
    pub fn declared_method_names(&self, interface_ref: &str) -> Option<Vec<String>> {
        self.get_interface_methods(interface_ref)
            .ok()
            .map(|ms| ms.into_iter().map(|m| m.name).collect())
    }

    /// MN2: could one implementation serve both of these?
    pub fn signatures_agree(&self, a: &MethodSig, b: &MethodSig) -> bool {
        self.signatures_match(a, b)
    }

    /// Get methods required by an interface.
    fn get_interface_methods(&self, interface_name: &str) -> Result<Vec<MethodSig>, InterfaceError> {
        // Strip generic args: "Iterator<i64>" → "Iterator"
        let base_name = interface_name.split('<').next().unwrap_or(interface_name);
        self.interface_methods
            .get(interface_name)
            .or_else(|| self.interface_methods.get(base_name))
            .cloned()
            .or_else(|| self.get_builtin_interface_methods(base_name))
            .ok_or_else(|| InterfaceError::UnknownInterface(interface_name.to_string()))
    }

    /// Get builtin interface methods for standard interfaces.
    fn get_builtin_interface_methods(&self, interface_name: &str) -> Option<Vec<MethodSig>> {
        builtin_interface_methods(interface_name)
    }
}

/// Signatures of an interface the compiler provides, with no type table needed.
///
/// Free-standing because the reachability pass needs the method *names* before
/// a type table exists, and duplicating the list there is how the two would
/// drift.
pub fn builtin_interface_methods(interface_name: &str) -> Option<Vec<MethodSig>> {
    {
        match interface_name {
            "Add" => Some(vec![MethodSig {
                owner_patterns: Vec::new(),
                type_params: Vec::new(),
                name: "add".to_string(),
                self_param: SelfParam::Value,
                params: vec![(Type::Var(crate::types::TypeVarId(0)), ParamMode::Default)], // Self type
                ret: Type::Var(crate::types::TypeVarId(0)),
            }]),
            "Sub" => Some(vec![MethodSig {
                owner_patterns: Vec::new(),
                type_params: Vec::new(),
                name: "sub".to_string(),
                self_param: SelfParam::Value,
                params: vec![(Type::Var(crate::types::TypeVarId(0)), ParamMode::Default)],
                ret: Type::Var(crate::types::TypeVarId(0)),
            }]),
            "Mul" => Some(vec![MethodSig {
                owner_patterns: Vec::new(),
                type_params: Vec::new(),
                name: "mul".to_string(),
                self_param: SelfParam::Value,
                params: vec![(Type::Var(crate::types::TypeVarId(0)), ParamMode::Default)],
                ret: Type::Var(crate::types::TypeVarId(0)),
            }]),
            "Div" => Some(vec![MethodSig {
                owner_patterns: Vec::new(),
                type_params: Vec::new(),
                name: "div".to_string(),
                self_param: SelfParam::Value,
                params: vec![(Type::Var(crate::types::TypeVarId(0)), ParamMode::Default)],
                ret: Type::Var(crate::types::TypeVarId(0)),
            }]),
            "Rem" => Some(vec![MethodSig {
                owner_patterns: Vec::new(),
                type_params: Vec::new(),
                name: "rem".to_string(),
                self_param: SelfParam::Value,
                params: vec![(Type::Var(crate::types::TypeVarId(0)), ParamMode::Default)],
                ret: Type::Var(crate::types::TypeVarId(0)),
            }]),
            "Neg" => Some(vec![MethodSig {
                owner_patterns: Vec::new(),
                type_params: Vec::new(),
                name: "neg".to_string(),
                self_param: SelfParam::Value,
                params: vec![],
                ret: Type::Var(crate::types::TypeVarId(0)),
            }]),
            "Equal" | "Eq" => Some(vec![MethodSig {
                owner_patterns: Vec::new(),
                type_params: Vec::new(),
                name: "eq".to_string(),
                self_param: SelfParam::Value,
                params: vec![(Type::Var(crate::types::TypeVarId(0)), ParamMode::Default)],
                ret: Type::Bool,
            }]),
            "Comparable" | "Ord" => Some(vec![
                MethodSig {
                    owner_patterns: Vec::new(),
                    type_params: Vec::new(),
                    name: "compare".to_string(),
                    self_param: SelfParam::Value,
                    params: vec![(Type::Var(crate::types::TypeVarId(0)), ParamMode::Default)],
                    // Type variable 0 is `Self` throughout these signatures,
                    // so `compare` has to name Ordering outright — as a
                    // placeholder it read as "returns Self", and a nominal
                    // newtype inheriting Comparable got a `compare` that
                    // claimed to answer with itself (#551).
                    ret: Type::UnresolvedNamed("Ordering".to_string()),
                },
                MethodSig {
                    owner_patterns: Vec::new(),
                    type_params: Vec::new(),
                    name: "lt".to_string(),
                    self_param: SelfParam::Value,
                    params: vec![(Type::Var(crate::types::TypeVarId(0)), ParamMode::Default)],
                    ret: Type::Bool,
                },
                MethodSig {
                    owner_patterns: Vec::new(),
                    type_params: Vec::new(),
                    name: "le".to_string(),
                    self_param: SelfParam::Value,
                    params: vec![(Type::Var(crate::types::TypeVarId(0)), ParamMode::Default)],
                    ret: Type::Bool,
                },
                MethodSig {
                    owner_patterns: Vec::new(),
                    type_params: Vec::new(),
                    name: "gt".to_string(),
                    self_param: SelfParam::Value,
                    params: vec![(Type::Var(crate::types::TypeVarId(0)), ParamMode::Default)],
                    ret: Type::Bool,
                },
                MethodSig {
                    owner_patterns: Vec::new(),
                    type_params: Vec::new(),
                    name: "ge".to_string(),
                    self_param: SelfParam::Value,
                    params: vec![(Type::Var(crate::types::TypeVarId(0)), ParamMode::Default)],
                    ret: Type::Bool,
                },
            ]),
            "Clone" | "Cloneable" => Some(vec![MethodSig {
                owner_patterns: Vec::new(),
                type_params: Vec::new(),
                name: "clone".to_string(),
                self_param: SelfParam::Value,
                params: vec![],
                ret: Type::Var(crate::types::TypeVarId(0)),
            }]),
            "Default" => Some(vec![MethodSig {
                owner_patterns: Vec::new(),
                type_params: Vec::new(),
                name: "default".to_string(),
                self_param: SelfParam::None, // Static method
                params: vec![],
                ret: Type::Var(crate::types::TypeVarId(0)),
            }]),
            "Hashable" => Some(vec![
                MethodSig {
                    owner_patterns: Vec::new(),
                    type_params: Vec::new(),
                    name: "hash".to_string(),
                    self_param: SelfParam::Value,
                    params: vec![],
                    ret: Type::U64,
                },
                MethodSig {
                    owner_patterns: Vec::new(),
                    type_params: Vec::new(),
                    name: "eq".to_string(),
                    self_param: SelfParam::Value,
                    params: vec![(Type::Var(crate::types::TypeVarId(0)), ParamMode::Default)],
                    ret: Type::Bool,
                },
            ]),
            "Displayable" => Some(vec![MethodSig {
                owner_patterns: Vec::new(),
                type_params: Vec::new(),
                name: "to_string".to_string(),
                self_param: SelfParam::Value,
                params: vec![],
                ret: Type::String,
            }]),
            "Debug" => Some(vec![MethodSig {
                owner_patterns: Vec::new(),
                type_params: Vec::new(),
                name: "debug".to_string(),
                self_param: SelfParam::Value,
                params: vec![],
                ret: Type::String,
            }]),
            // Iterator<Item> interface — single method `next(mutate self) -> Item?`
            "Iterator" => Some(vec![MethodSig {
                owner_patterns: Vec::new(),
                type_params: Vec::new(),
                name: "next".to_string(),
                self_param: SelfParam::Mutate,
                params: vec![],
                ret: Type::option(Type::Var(crate::types::TypeVarId(0))),
            }]),
            // NT1–NT3 / the standard-interface roster. `Numeric` is a nominal
            // interface with these eight; `Integer` and `Float` extend it with
            // constants, which have no MethodSig to stand for them — a
            // primitive answers through the membership test above, and a user
            // type is held to the methods it can actually declare.
            "Numeric" => Some(numeric_method_sigs()),
            // `Integer` gets the shared eight plus the overflow-hatch methods
            // (type.integer-overflow M1) — floats have no wrapping/saturating
            // semantics, so these don't belong on `Numeric` itself. Needed so
            // a generic body bounded by `T: Integer` (e.g. `Wrapping<T>`'s
            // `add`) can call `.wrapping_add()` on its receiver — without this
            // the concrete-type methods exist but a generic `T` can't see
            // them (#838).
            "Integer" => {
                let mut sigs = numeric_method_sigs();
                sigs.extend(integer_overflow_hatch_method_sigs());
                sigs.extend(ordered_method_sigs());
                Some(sigs)
            }
            "Float" => {
                let mut sigs = numeric_method_sigs();
                sigs.extend(ordered_method_sigs());
                sigs.push(MethodSig {
                    owner_patterns: Vec::new(),
                    type_params: Vec::new(),
                    name: "is_nan".to_string(),
                    self_param: SelfParam::Value,
                    params: vec![],
                    ret: Type::Bool,
                });
                Some(sigs)
            }
            // ER4/ER32: the Error interface — `func message(self) -> string`
            "Error" => Some(vec![MethodSig {
                owner_patterns: Vec::new(),
                type_params: Vec::new(),
                name: "message".to_string(),
                self_param: SelfParam::Value,
                params: vec![],
                ret: Type::String,
            }]),
            _ => None,
        }
    }
}

/// Object-compatible method names of a compiler-provided interface, in vtable
/// order. Empty for an interface the compiler doesn't provide.
pub fn builtin_interface_method_names(interface_name: &str) -> Vec<String> {
    builtin_interface_methods(interface_name)
        .unwrap_or_default()
        .into_iter()
        .filter(|m| {
            m.type_params.is_empty()
                && !matches!(&m.ret, Type::UnresolvedNamed(n) if n == "Self")
        })
        .map(|m| m.name)
        .collect()
}

impl<'a> InterfaceChecker<'a> {
    /// Get methods available on a type.
    fn get_type_methods(&self, ty: &Type) -> Vec<MethodSig> {
        let id = match ty {
            Type::Named(id) => Some(*id),
            // A generic instantiation carries the base type's methods.
            Type::Generic { base, .. } => Some(*base),
            // A name that hasn't been resolved to `Named` yet (e.g. a stdlib
            // function's return type, parsed lazily from its stub string)
            // still names a registered struct/enum — look it up by name
            // rather than reporting it methodless.
            Type::UnresolvedNamed(name) => self.types.get_type_id(name),
            Type::UnresolvedGeneric { name, .. } => self.types.get_type_id(name),
            _ => None,
        };
        match id.and_then(|id| self.types.get(id)) {
            Some(TypeDef::Struct { methods, .. }) => methods.clone(),
            Some(TypeDef::Enum { methods, .. }) => methods.clone(),
            Some(TypeDef::Interface { methods, .. }) => methods.clone(),
            // T13: an `extend` block on a nominal type puts its methods on the
            // nominal type, which is where `register_impl_methods` writes them.
            // Left out here, `extend MyDoc implements Labeled { func label … }` came
            // back methodless and G1 reported every interface method missing on a
            // block that had them all — so the newtype, which is the way out of
            // both XC1 and XC3, couldn't carry a conformance at all.
            Some(TypeDef::NominalAlias { methods, .. }) => methods.clone(),
            // Primitives and unions have builtin methods checked separately.
            _ => Vec::new(),
        }
    }

    /// Check if a primitive type has a builtin method.
    fn has_builtin_method(&self, ty: &Type, method: &str) -> bool {
        match ty {
            // Integer types: eq, hash, clone, default, arithmetic, compare, to_string
            Type::I8 | Type::I16 | Type::I32 | Type::I64 | Type::I128 |
            Type::U8 | Type::U16 | Type::U32 | Type::U64 | Type::U128 => {
                matches!(method,
                    "add" | "sub" | "mul" | "div" | "rem" |
                    "neg" | "eq" | "lt" | "le" | "gt" | "ge" | "compare" |
                    "bit_and" | "bit_or" | "bit_xor" | "shl" | "shr" | "bit_not" |
                    "hash" | "clone" | "default" | "to_string" | "debug"
                )
            }
            // Floats: eq, clone, default, but NOT hash (HA4)
            Type::F32 | Type::F64 => {
                matches!(method,
                    "add" | "sub" | "mul" | "div" | "rem" |
                    "neg" | "eq" | "lt" | "le" | "gt" | "ge" | "compare" |
                    "bit_and" | "bit_or" | "bit_xor" | "shl" | "shr" | "bit_not" |
                    "clone" | "default" | "to_string" | "debug"
                )
            }
            // Bool: eq, hash, clone, default, compare, to_string
            Type::Bool => matches!(method, "eq" | "compare" | "hash" | "clone" | "default" | "to_string" | "debug"),
            // Char: eq, hash, clone, default, comparison, to_string
            Type::Char => matches!(method, "eq" | "lt" | "le" | "gt" | "ge" | "compare" | "hash" | "clone" | "default" | "to_string" | "debug"),
            // String: eq, hash, clone, default, len, comparison, to_string
            Type::String => matches!(method, "eq" | "lt" | "le" | "gt" | "ge" | "compare" | "len" | "clone" | "hash" | "default" | "to_string" | "debug"),
            // Unit: eq, hash, clone, default
            Type::Unit => matches!(method, "eq" | "hash" | "clone" | "default" | "to_string" | "debug"),
            _ => false,
        }
    }

    /// Check if two method signatures match.
    fn signatures_match(&self, required: &MethodSig, found: &MethodSig) -> bool {
        if required.self_param != found.self_param {
            return false;
        }

        if required.params.len() != found.params.len() {
            return false;
        }

        // Check parameter modes and types per position.
        // Type::Var represents Self in builtin interface signatures, and
        // `UnresolvedNamed("Self")` is the written-out Self of a declared
        // interface — both stand in for the implementing type, so skip the type
        // comparison when either side is one of those.
        for ((req_ty, req_mode), (found_ty, found_mode)) in
            required.params.iter().zip(found.params.iter())
        {
            if req_mode != found_mode {
                return false;
            }
            if !is_self_placeholder(req_ty)
                && !matches!(found_ty, Type::Var(_))
                && !self.types_equivalent(req_ty, found_ty)
            {
                return false;
            }
        }

        // Check return type (skip Self placeholders)
        if !is_self_placeholder(&required.ret)
            && !matches!(found.ret, Type::Var(_))
            && !self.types_equivalent(&required.ret, &found.ret)
        {
            return false;
        }

        true
    }

    /// Do two written-out types name the same type?
    ///
    /// A name that wasn't registered yet when its signature was read stays an
    /// `UnresolvedNamed`, and comparing that to the registered `Named` it means
    /// says they differ. So an interface whose method returned a type declared
    /// further down the file rejected every implementation of it — declaration
    /// order decided whether a conformance held.
    fn types_equivalent(&self, a: &Type, b: &Type) -> bool {
        self.normalize(a) == self.normalize(b)
    }

    fn normalize(&self, ty: &Type) -> Type {
        match ty {
            Type::UnresolvedNamed(name) => {
                self.types.lookup(name).unwrap_or_else(|| ty.clone())
            }
            Type::UnresolvedGeneric { name, args } => match self.types.get_type_id(name) {
                Some(base) => Type::Generic {
                    base,
                    args: args.iter().map(|a| self.normalize_arg(a)).collect(),
                },
                None => Type::UnresolvedGeneric {
                    name: name.clone(),
                    args: args.iter().map(|a| self.normalize_arg(a)).collect(),
                },
            },
            Type::Generic { base, args } => Type::Generic {
                base: *base,
                args: args.iter().map(|a| self.normalize_arg(a)).collect(),
            },
            Type::Result { ok, err } => Type::Result {
                ok: Box::new(self.normalize(ok)),
                err: Box::new(self.normalize(err)),
            },
            Type::Tuple(elems) => Type::Tuple(elems.iter().map(|e| self.normalize(e)).collect()),
            Type::RawPtr(inner) => Type::RawPtr(Box::new(self.normalize(inner))),
            Type::Array { elem, len } => Type::Array {
                elem: Box::new(self.normalize(elem)),
                len: *len,
            },
            Type::Union(parts) => Type::Union(parts.iter().map(|p| self.normalize(p)).collect()),
            other => other.clone(),
        }
    }

    fn normalize_arg(&self, arg: &GenericArg) -> GenericArg {
        match arg {
            GenericArg::Type(t) => GenericArg::Type(Box::new(self.normalize(t))),
            other => other.clone(),
        }
    }

    /// Format a method signature for error messages.
    /// A signature as a reader would write it: `func scale(self, f64) -> Meters`.
    ///
    /// Used to print the checker's own `Debug` — `fn scale(self, I64) ->
    /// Named(TypeId(104))` — which names a type by its slot in a table nobody
    /// outside the compiler can see.
    fn format_signature(&self, sig: &MethodSig) -> String {
        // The separator belongs between the receiver and the first parameter,
        // not after the receiver — `func hash(self, ) -> u64` is what it read
        // for a method that takes nothing else.
        let sep = if sig.params.is_empty() { "" } else { ", " };
        let self_str = match sig.self_param {
            SelfParam::None => String::new(),
            SelfParam::Value => format!("self{}", sep),
            SelfParam::Mutate => format!("mutate self{}", sep),
            SelfParam::Take => format!("take self{}", sep),
        };
        let params_str: Vec<String> = sig.params.iter().map(|(t, mode)| {
            match mode {
                ParamMode::Take => format!("take {}", self.type_name(t)),
                ParamMode::Mutate => format!("mutate {}", self.type_name(t)),
                ParamMode::Default => self.type_name(t),
            }
        }).collect();
        let base = sig.name.split('<').next().unwrap_or(&sig.name);
        let ret = self.type_name(&sig.ret);
        if ret == "()" {
            return format!("func {}({}{})", base, self_str, params_str.join(", "));
        }
        format!("func {}({}{}) -> {}", base, self_str, params_str.join(", "), ret)
    }

    /// Get a human-readable name for a type.
    fn type_name(&self, ty: &Type) -> String {
        match ty {
            Type::Named(id) => {
                if let Some(def) = self.types.get(*id) {
                    match def {
                        TypeDef::Struct { name, .. } => name.clone(),
                        TypeDef::Enum { name, .. } => name.clone(),
                        TypeDef::Interface { name, .. } => name.clone(),
                        TypeDef::Union { name, .. } => name.clone(),
                        TypeDef::NominalAlias { name, .. } => name.clone(),
                        TypeDef::Primitive { name, .. } => name.clone(),
                    }
                } else {
                    format!("Type({})", id.0)
                }
            }
            Type::Unit => "()".to_string(),
            // A placeholder standing in for the implementing type, in both the
            // spellings that reach here (a declared interface writes `Self`, a
            // compiler-provided one uses a type variable).
            Type::Var(_) => "Self".to_string(),
            Type::Generic { base, args } => {
                let inner: Vec<String> = args
                    .iter()
                    .map(|a| match a {
                        GenericArg::Type(t) => self.type_name(t),
                        other => format!("{}", other),
                    })
                    .collect();
                format!("{}<{}>", self.types.type_name(*base), inner.join(", "))
            }
            Type::UnresolvedGeneric { name, args } => {
                let inner: Vec<String> = args
                    .iter()
                    .map(|a| match a {
                        GenericArg::Type(t) => self.type_name(t),
                        other => format!("{}", other),
                    })
                    .collect();
                format!("{}<{}>", name, inner.join(", "))
            }
            Type::UnresolvedNamed(name) => name.clone(),
            Type::Tuple(elems) => {
                let inner: Vec<String> = elems.iter().map(|e| self.type_name(e)).collect();
                format!("({})", inner.join(", "))
            }
            Type::Result { ok, err } if matches!(**err, Type::None) => {
                format!("{}?", self.type_name(ok))
            }
            Type::Result { ok, err } => {
                format!("{} or {}", self.type_name(ok), self.type_name(err))
            }
            Type::Array { elem, len } => format!("[{}; {}]", self.type_name(elem), len),
            Type::RawPtr(inner) => format!("*{}", self.type_name(inner)),
            _ => format!("{}", ty),
        }
    }

    /// Consume the checker and return any errors.
    pub fn into_errors(self) -> Vec<InterfaceError> {
        self.errors
    }
}

// ============================================================================
// Interface Satisfaction Verification
// ============================================================================

/// Verify interface satisfaction at a generic instantiation site.
pub fn verify_instantiation(
    types: &TypeTable,
    concrete_type: &Type,
    bounds: &[InterfaceBound],
    span: Span,
) -> Result<(), Vec<InterfaceError>> {
    let mut checker = InterfaceChecker::new(types);
    let errors = checker.check_bounds(concrete_type, bounds, span);

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Check if a type implements a specific interface.
/// True if `ty` stands in for the implementing type in an interface signature:
/// a builtin-interface type variable, or the written-out `Self`.
fn is_self_placeholder(ty: &Type) -> bool {
    matches!(ty, Type::Var(_)) || matches!(ty, Type::UnresolvedNamed(n) if n == "Self")
}

/// A generic argument that isn't a concrete type yet — an inference var or a
/// bare type parameter. CC1 conditions on these are deferred until concrete.
fn is_abstract_arg(ty: &Type) -> bool {
    match ty {
        Type::Var(_) | Type::Error => true,
        Type::UnresolvedNamed(n) => {
            let mut chars = n.chars();
            matches!((chars.next(), chars.next()), (Some(c), None) if c.is_ascii_uppercase())
        }
        _ => false,
    }
}

/// The wrapping/saturating one-off methods (type.integer-overflow M1) that
/// `Integer` adds on top of `Numeric`'s eight. Same shape as `Numeric`'s
/// binary operators — one operand of the receiver's own type, same type back
/// — since each is the checked operator with the panic swapped for wrap or
/// saturate.
fn integer_overflow_hatch_method_sigs() -> Vec<MethodSig> {
    let binary = |name: &str| MethodSig {
        owner_patterns: Vec::new(),
        type_params: Vec::new(),
        name: name.to_string(),
        self_param: SelfParam::Value,
        params: vec![(Type::Var(crate::types::TypeVarId(0)), ParamMode::Default)],
        ret: Type::Var(crate::types::TypeVarId(0)),
    };
    vec![
        binary("wrapping_add"),
        binary("wrapping_sub"),
        binary("wrapping_mul"),
        binary("saturating_add"),
        binary("saturating_sub"),
        binary("saturating_mul"),
    ]
}

/// The eight methods the roster gives `Numeric`.
/// Comparison and remainder — what every `Integer` and `Float` member has and
/// `Numeric` alone doesn't declare.
///
/// A bound has to declare a method for a generic body to call it, and membership
/// in these two sets is decided by what the type *is* — so a `T: Integer` that
/// couldn't be compared was the bound describing less than it knows. That's what
/// stopped `stdlib/range.rk` from asking `self.start < self.end`.
fn ordered_method_sigs() -> Vec<MethodSig> {
    let mut sigs = builtin_interface_methods("Comparable").unwrap_or_default();
    sigs.extend(builtin_interface_methods("Equal").unwrap_or_default());
    sigs.extend(builtin_interface_methods("Rem").unwrap_or_default());
    sigs
}

fn numeric_method_sigs() -> Vec<MethodSig> {
    let binary = |name: &str| MethodSig {
        owner_patterns: Vec::new(),
        type_params: Vec::new(),
        name: name.to_string(),
        self_param: SelfParam::Value,
        params: vec![(Type::Var(crate::types::TypeVarId(0)), ParamMode::Default)],
        ret: Type::Var(crate::types::TypeVarId(0)),
    };
    let nullary = |name: &str, self_param| MethodSig {
        owner_patterns: Vec::new(),
        type_params: Vec::new(),
        name: name.to_string(),
        self_param,
        params: vec![],
        ret: Type::Var(crate::types::TypeVarId(0)),
    };
    vec![
        binary("add"),
        binary("sub"),
        binary("mul"),
        binary("div"),
        nullary("neg", SelfParam::Value),
        nullary("zero", SelfParam::None),
        nullary("one", SelfParam::None),
        MethodSig {
            owner_patterns: Vec::new(),
            type_params: Vec::new(),
            name: "from_int".to_string(),
            self_param: SelfParam::None,
            params: vec![(Type::I64, ParamMode::Default)],
            ret: Type::Var(crate::types::TypeVarId(0)),
        },
    ]
}

/// Membership test for one of the numeric interfaces, or `None` if `name` isn't
/// one of them.
///
/// NT2/NT3 spell these as interfaces over associated constants. Nothing declares
/// them and nothing can implement them — a type is a member because of what it
/// is, so the bound is a set test.
fn numeric_interface_members(name: &str) -> Option<fn(&Type) -> bool> {
    match name {
        "Integer" => Some(is_integer_type),
        "Float" => Some(is_float_type),
        "Numeric" => Some(|ty: &Type| is_integer_type(ty) || is_float_type(ty)),
        _ => None,
    }
}

fn is_integer_type(ty: &Type) -> bool {
    matches!(
        ty,
        Type::I8 | Type::I16 | Type::I32 | Type::I64 | Type::I128
            | Type::U8 | Type::U16 | Type::U32 | Type::U64 | Type::U128
    )
}

fn is_float_type(ty: &Type) -> bool {
    matches!(ty, Type::F32 | Type::F64)
}

/// Interfaces the compiler provides rather than the program declaring them.
///
/// A vtable can only be built for an interface whose method list is known, and both
/// places that build one — the reachability pass and the CLI's vtable
/// collection — read that list off `interface Foo { … }` declarations. A
/// compiler-provided interface has no declaration, so `any Error` boxed fine
/// and then had nothing to dispatch through: MIR skipped the vtable path and
/// fell back to static dispatch, which failed lowering with "method `message`
/// on receiver of unresolved type" (#708).
pub const COMPILER_PROVIDED_TRAITS: [&str; 4] =
    ["Error", "Displayable", "Debug", "Hashable"];

/// Method names an interface object of `interface_name` can dispatch, in vtable order.
///
/// Declared interfaces answer from their declaration, compiler-provided ones from
/// the builtin table. Object compatibility applies either way (TR2/TR3): a
/// method with its own type parameters, or one returning `Self`, has no vtable
/// slot.
pub fn object_compatible_methods(types: &TypeTable, interface_name: &str) -> Vec<String> {
    object_compatible_methods_seen(types, interface_name, &mut Vec::new())
}

/// `object_compatible_methods`, refusing to walk a super-interface twice — a cycle
/// in the graph would otherwise recurse forever.
fn object_compatible_methods_seen(
    types: &TypeTable,
    interface_name: &str,
    seen: &mut Vec<String>,
) -> Vec<String> {
    let base = interface_name.split('<').next().unwrap_or(interface_name);
    if seen.iter().any(|s| s == base) {
        return Vec::new();
    }
    seen.push(base.to_string());
    if let Some(def) = types.get_type_id(base).and_then(|id| types.get(id)) {
        let mut names = def.object_compatible_method_names();
        if !names.is_empty() {
            // A super-interface's methods are part of the sub-interface's contract, so
            // they need vtable slots of their own. `interface Shouty: Speak` listed
            // only `shout`, so `x.say()` on an `any Shouty` had no slot to
            // dispatch through — MIR gave up on it while the interpreter, which
            // looks the method up by name, ran it (#873).
            //
            // Declared methods first, then inherited, which is the order the
            // checker's own merge uses. Both the vtable layout and MIR's
            // dispatch offsets read this one list, so they agree by
            // construction whatever the order is.
            if let TypeDef::Interface { super_interfaces, .. } = def {
                for parent in super_interfaces {
                    for m in object_compatible_methods_seen(types, parent, seen) {
                        if !names.contains(&m) {
                            names.push(m);
                        }
                    }
                }
            }
            return names;
        }
    }
    let checker = InterfaceChecker::new(types);
    checker
        .get_interface_methods_public(base)
        .into_iter()
        .filter(|m| {
            m.type_params.is_empty()
                && !matches!(&m.ret, Type::UnresolvedNamed(n) if n == "Self")
        })
        .map(|m| m.name)
        .collect()
}

pub fn implements_interface(
    types: &TypeTable,
    ty: &Type,
    interface_name: &str,
) -> bool {
    let mut checker = InterfaceChecker::new(types);
    checker.check_satisfies(ty, interface_name, Span::new(0, 0)).is_ok()
}

/// Get all interfaces that a type implements.
pub fn implemented_interfaces(types: &TypeTable, ty: &Type) -> Vec<String> {
    let mut result = Vec::new();
    // Check against known interfaces
    let known_interfaces = [
        "Add", "Sub", "Mul", "Div", "Rem", "Neg",
        "Equal", "Eq", "Comparable", "Ord",
        "Clone", "Cloneable", "Default", "Hashable",
        "Displayable", "Debug",
    ];

    for interface_name in known_interfaces {
        let mut checker = InterfaceChecker::new(types);
        if checker.check_satisfies(ty, interface_name, Span::new(0, 0)).is_ok() {
            result.push(interface_name.to_string());
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_primitive_interface_satisfaction() {
        let types = TypeTable::new();

        // i32 should implement Add
        assert!(implements_interface(&types, &Type::I32, "Add"));
        assert!(implements_interface(&types, &Type::I32, "Equal"));
        assert!(implements_interface(&types, &Type::I32, "Comparable"));
    }

    // CC1: `extend Ring<T> implements Show where T: Show` — the conformance holds for
    // Ring<Coin> (Coin: Show) and fails for Ring<Blob> (Blob not Show).
    #[test]
    fn conditional_conformance_checks_argument() {
        use crate::checker::{MethodSig, SelfParam};
        use crate::types::GenericArg;
        use rask_ast::Span;

        let mut types = TypeTable::new();
        let show = || MethodSig {
            owner_patterns: Vec::new(),
            type_params: Vec::new(),
            name: "show".to_string(),
            self_param: SelfParam::Value,
            params: vec![],
            ret: Type::String,
        };

        types.register_type(TypeDef::Interface {
            type_params: Vec::new(),
            assoc_types: Vec::new(),
            name: "Show".to_string(),
            super_interfaces: vec![],
            methods: vec![show()],
            generic_methods: vec![],
            is_unsafe: false,
            is_duck: false,
        });
        let ring = types.register_type(TypeDef::Struct {
            name: "Ring".to_string(),
            type_params: vec!["T".to_string()],
            fields: vec![],
            methods: vec![show()],
            is_resource: false,
            is_unique: false,
            is_binary: false,
            private_fields: vec![],
            skipped_fields: vec![],
            undecodable_fields: vec![],
            is_transitive_resource: false,
            no_encode: false,
            no_decode: false,
        });
        let coin = types.register_type(TypeDef::Struct {
            name: "Coin".to_string(),
            type_params: vec![],
            fields: vec![],
            methods: vec![show()],
            is_resource: false,
            is_unique: false,
            is_binary: false,
            private_fields: vec![],
            skipped_fields: vec![],
            undecodable_fields: vec![],
            is_transitive_resource: false,
            no_encode: false,
            no_decode: false,
        });
        let blob = types.register_type(TypeDef::Struct {
            name: "Blob".to_string(),
            type_params: vec![],
            fields: vec![],
            methods: vec![],
            is_resource: false,
            is_unique: false,
            is_binary: false,
            private_fields: vec![],
            skipped_fields: vec![],
            undecodable_fields: vec![],
            is_transitive_resource: false,
            no_encode: false,
            no_decode: false,
        });

        // extend Ring<T> implements Show where T: Show
        types.record_conformance(ring, "Show");
        types.record_conformance_condition(ring, "Show", vec![("T".to_string(), vec!["Show".to_string()])]);
        // extend Coin implements Show
        types.record_conformance(coin, "Show");

        let ring_of = |arg: crate::types::TypeId| Type::Generic {
            base: ring,
            args: vec![GenericArg::Type(Box::new(Type::Named(arg)))],
        };

        let mut checker = InterfaceChecker::new(&types);
        assert!(checker.check_satisfies(&ring_of(coin), "Show", Span::new(0, 0)).is_ok(),
            "Ring<Coin> should satisfy Show (Coin: Show)");
        assert!(checker.check_satisfies(&ring_of(blob), "Show", Span::new(0, 0)).is_err(),
            "Ring<Blob> must NOT satisfy Show (Blob is not Show)");
    }
}

/// Rewrite a required signature under a conformance's substitution (GT2/AT6).
pub fn substitute_signature(m: &MethodSig, map: &HashMap<String, Type>) -> MethodSig {
    if map.is_empty() {
        return m.clone();
    }
    MethodSig {
        owner_patterns: m.owner_patterns.clone(),
        type_params: m.type_params.clone(),
        name: m.name.clone(),
        self_param: m.self_param,
        params: m
            .params
            .iter()
            .map(|(t, mode)| (substitute_type(t, map), *mode))
            .collect(),
        ret: substitute_type(&m.ret, map),
    }
}

/// Replace every written name the substitution covers. A name it doesn't cover
/// is left alone — an unbound `Rhs` or an unsupplied `Self.Out` is reported
/// where it was written, not silently turned into something else.
pub fn substitute_type(ty: &Type, map: &HashMap<String, Type>) -> Type {
    match ty {
        Type::UnresolvedNamed(name) => map.get(name).cloned().unwrap_or_else(|| ty.clone()),
        // AT3: `Self.Out`. The base is substituted first so a projection on a
        // interface parameter (`Rhs.Out`) follows the same path.
        Type::Assoc { base, name } => {
            let base = substitute_type(base, map);
            let key = format!("{}.{}", base, name);
            if let Some(t) = map.get(&key) {
                return t.clone();
            }
            Type::Assoc { base: Box::new(base), name: name.clone() }
        }
        Type::UnresolvedGeneric { name, args } => {
            let args: Vec<GenericArg> = args
                .iter()
                .map(|a| match a {
                    GenericArg::Type(t) => GenericArg::Type(Box::new(substitute_type(t, map))),
                    other => other.clone(),
                })
                .collect();
            match map.get(name) {
                // `Vec` never stands in for a parameter, but a parameter used
                // bare with arguments would — keep the base if it's mapped.
                Some(Type::UnresolvedNamed(n)) => Type::UnresolvedGeneric { name: n.clone(), args },
                _ => Type::UnresolvedGeneric { name: name.clone(), args },
            }
        }
        Type::Generic { base, args } => Type::Generic {
            base: *base,
            args: args
                .iter()
                .map(|a| match a {
                    GenericArg::Type(t) => GenericArg::Type(Box::new(substitute_type(t, map))),
                    other => other.clone(),
                })
                .collect(),
        },
        Type::Tuple(elems) => Type::Tuple(elems.iter().map(|e| substitute_type(e, map)).collect()),
        Type::Array { elem, len } => Type::Array {
            elem: Box::new(substitute_type(elem, map)),
            len: *len,
        },
        Type::Result { ok, err } => Type::Result {
            ok: Box::new(substitute_type(ok, map)),
            err: Box::new(substitute_type(err, map)),
        },
        Type::Union(parts) => Type::Union(parts.iter().map(|p| substitute_type(p, map)).collect()),
        Type::Fn { params, ret } => Type::Fn {
            params: params.iter().map(|p| substitute_type(p, map)).collect(),
            ret: Box::new(substitute_type(ret, map)),
        },
        _ => ty.clone(),
    }
}

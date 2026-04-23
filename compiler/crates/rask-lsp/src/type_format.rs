// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Type formatting for hover tooltips and completion details.

use rask_types::{GenericArg, Type, TypeTable};

/// Formats types for human-readable display in hover tooltips.
pub struct TypeFormatter<'a> {
    types: &'a TypeTable,
}

impl<'a> TypeFormatter<'a> {
    pub fn new(types: &'a TypeTable) -> Self {
        Self { types }
    }

    pub fn format(&self, ty: &Type) -> String {
        match ty {
            Type::Unit => "void".to_string(),
            Type::Never => "!".to_string(),
            Type::Bool => "bool".to_string(),
            Type::I8 => "i8".to_string(),
            Type::I16 => "i16".to_string(),
            Type::I32 => "i32".to_string(),
            Type::I64 => "i64".to_string(),
            Type::I128 => "i128".to_string(),
            Type::U8 => "u8".to_string(),
            Type::U16 => "u16".to_string(),
            Type::U32 => "u32".to_string(),
            Type::U64 => "u64".to_string(),
            Type::U128 => "u128".to_string(),
            Type::F32 => "f32".to_string(),
            Type::F64 => "f64".to_string(),
            Type::String => "string".to_string(),
            Type::Char => "char".to_string(),

            Type::Named(id) => {
                self.types.type_name(*id)
            }

            Type::Generic { base, args } => {
                let base_name = self.types.type_name(*base);
                let args_str = args.iter()
                    .map(|t| self.format_generic_arg(t))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{}<{}>", base_name, args_str)
            }

            Type::UnresolvedGeneric { name, args } => {
                if args.is_empty() {
                    name.clone()
                } else {
                    let args_str = args.iter()
                        .map(|t| self.format_generic_arg(t))
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!("{}<{}>", name, args_str)
                }
            }

            Type::Fn { params, ret } => {
                let params_str = params.iter()
                    .map(|p| self.format(p))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("func({}) -> {}", params_str, self.format(ret))
            }

            Type::Option(inner) => format!("{}?", self.format(inner)),

            Type::Result { ok, err } => {
                format!("{} or {}", self.format(ok), self.format(err))
            }

            Type::Tuple(elements) => {
                let elems_str = elements.iter()
                    .map(|e| self.format(e))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("({})", elems_str)
            }

            Type::Array { elem, len } => format!("[{}; {}]", self.format(elem), len),
            Type::Slice(elem) => format!("[{}]", self.format(elem)),
            Type::RawPtr(inner) => format!("*{}", self.format(inner)),
            Type::Union(types) => {
                types.iter()
                    .map(|t| self.format(t))
                    .collect::<Vec<_>>()
                    .join(" | ")
            }
            Type::SimdVector { elem, lanes } => format!("{}x{}", self.format(elem), lanes),
            Type::TraitObject { trait_name } => format!("any {}", trait_name),
            Type::Var(_) => "_".to_string(),
            Type::UnresolvedNamed(name) => name.clone(),
            Type::Error => "<error>".to_string(),
        }
    }

    pub fn format_generic_arg(&self, arg: &GenericArg) -> String {
        match arg {
            GenericArg::Type(ty) => self.format(ty),
            GenericArg::ConstUsize(n) => n.to_string(),
        }
    }
}

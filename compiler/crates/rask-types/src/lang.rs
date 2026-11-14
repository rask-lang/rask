// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! The stdlib declarations the compiler needs by identity. Each is marked
//! where it is declared, `@lang(field_info) public struct FieldInfo`, and the
//! checker files its `TypeId` once while registering declarations. Every pass
//! after asks for the item, never for a name.

/// One declaration the compiler relies on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LangItem {
    /// What `compare` returns. The checker registers it; nothing declares it.
    Ordering,
    FieldInfo,
    MethodInfo,
    VariantInfo,
    ParamInfo,
    JsonValue,
    Choices,
    /// `async.Handle<T>`, what a task block hands back.
    TaskHandle,
}

impl LangItem {
    /// The item an `@lang(…)` argument names.
    pub fn from_attr(name: &str) -> Option<Self> {
        Some(match name {
            "ordering" => LangItem::Ordering,
            "field_info" => LangItem::FieldInfo,
            "method_info" => LangItem::MethodInfo,
            "variant_info" => LangItem::VariantInfo,
            "param_info" => LangItem::ParamInfo,
            "json_value" => LangItem::JsonValue,
            "choices" => LangItem::Choices,
            "task_handle" => LangItem::TaskHandle,
            _ => return None,
        })
    }
}

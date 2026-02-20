// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Output types for `rask describe`.

use serde::Serialize;

/// Complete module description.
#[derive(Debug, Serialize)]
pub struct ModuleDescription {
    pub version: u32,
    pub module: String,
    pub file: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub imports: Vec<ImportDesc>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub types: Vec<StructDesc>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub enums: Vec<EnumDesc>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub traits: Vec<TraitDesc>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub functions: Vec<FunctionDesc>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub constants: Vec<ConstantDesc>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub externs: Vec<ExternDesc>,
}

/// Function or method description.
#[derive(Debug, Serialize)]
pub struct FunctionDesc {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doc: Option<String>,
    pub public: bool,
    pub params: Vec<ParamDesc>,
    pub returns: ReturnsDesc,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub self_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub type_params: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attrs: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r#unsafe: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comptime: Option<bool>,
}

/// Function parameter.
#[derive(Debug, Serialize)]
pub struct ParamDesc {
    pub name: String,
    #[serde(rename = "type")]
    pub type_str: String,
    pub mode: String,
}

/// Return type, split into ok/err for Result types.
#[derive(Debug, Serialize)]
pub struct ReturnsDesc {
    pub ok: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub err: Option<String>,
}

/// Struct type description.
#[derive(Debug, Serialize)]
pub struct StructDesc {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doc: Option<String>,
    pub public: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub type_params: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attrs: Option<Vec<String>>,
    pub fields: Vec<FieldDesc>,
    pub methods: Vec<FunctionDesc>,
}

/// Struct/variant field.
#[derive(Debug, Serialize)]
pub struct FieldDesc {
    pub name: String,
    #[serde(rename = "type")]
    pub type_str: String,
    pub public: bool,
}

/// Enum type description.
#[derive(Debug, Serialize)]
pub struct EnumDesc {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doc: Option<String>,
    pub public: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub type_params: Option<Vec<String>>,
    pub variants: Vec<VariantDesc>,
    pub methods: Vec<FunctionDesc>,
}

/// Enum variant.
#[derive(Debug, Serialize)]
pub struct VariantDesc {
    pub name: String,
    pub fields: Vec<FieldDesc>,
}

/// Trait description.
#[derive(Debug, Serialize)]
pub struct TraitDesc {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doc: Option<String>,
    pub public: bool,
    pub methods: Vec<FunctionDesc>,
}

/// Import declaration.
#[derive(Debug, Serialize)]
pub struct ImportDesc {
    pub path: Vec<String>,
    pub alias: Option<String>,
    pub is_glob: bool,
    pub is_lazy: bool,
}

/// Top-level constant.
#[derive(Debug, Serialize)]
pub struct ConstantDesc {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doc: Option<String>,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub type_str: Option<String>,
    pub public: bool,
}

/// External function declaration.
#[derive(Debug, Serialize)]
pub struct ExternDesc {
    pub abi: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doc: Option<String>,
    pub params: Vec<ParamDesc>,
    pub returns: ReturnsDesc,
}

/// Options for describe output.
pub struct DescribeOpts {
    pub show_all: bool,
}

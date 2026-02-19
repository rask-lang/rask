// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Extract module description from parsed AST.

use std::collections::HashMap;
use std::path::Path;

use rask_ast::decl::*;

use crate::types::*;

/// Extract a module description from parsed declarations.
pub fn extract(decls: &[Decl], file: &str, opts: &DescribeOpts) -> ModuleDescription {
    let module_name = Path::new(file)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();

    // First pass: collect types by name for impl merging
    let mut struct_map: HashMap<String, usize> = HashMap::new();
    let mut enum_map: HashMap<String, usize> = HashMap::new();

    let mut types: Vec<StructDesc> = Vec::new();
    let mut enums: Vec<EnumDesc> = Vec::new();
    let mut traits: Vec<TraitDesc> = Vec::new();
    let mut functions: Vec<FunctionDesc> = Vec::new();
    let mut constants: Vec<ConstantDesc> = Vec::new();
    let mut imports: Vec<ImportDesc> = Vec::new();
    let mut externs: Vec<ExternDesc> = Vec::new();

    for decl in decls {
        match &decl.kind {
            DeclKind::Struct(s) => {
                if !opts.show_all && !s.is_pub {
                    continue;
                }
                let idx = types.len();
                struct_map.insert(s.name.clone(), idx);
                types.push(extract_struct(s, opts));
            }
            DeclKind::Enum(e) => {
                if !opts.show_all && !e.is_pub {
                    continue;
                }
                let idx = enums.len();
                enum_map.insert(e.name.clone(), idx);
                enums.push(extract_enum(e, opts));
            }
            DeclKind::Trait(t) => {
                if !opts.show_all && !t.is_pub {
                    continue;
                }
                traits.push(extract_trait(t));
            }
            DeclKind::Fn(f) => {
                if !opts.show_all && !f.is_pub {
                    continue;
                }
                functions.push(extract_function(f));
            }
            DeclKind::Const(c) => {
                if !opts.show_all && !c.is_pub {
                    continue;
                }
                constants.push(ConstantDesc {
                    name: c.name.clone(),
                    type_str: c.ty.clone(),
                    public: c.is_pub,
                });
            }
            DeclKind::Import(i) => {
                imports.push(ImportDesc {
                    path: i.path.clone(),
                    alias: i.alias.clone(),
                    is_glob: i.is_glob,
                    is_lazy: i.is_lazy,
                });
            }
            DeclKind::Extern(e) => {
                externs.push(extract_extern(e));
            }
            DeclKind::Impl(imp) => {
                // Merge methods into their target type
                let methods: Vec<FunctionDesc> = imp
                    .methods
                    .iter()
                    .filter(|m| opts.show_all || m.is_pub)
                    .map(extract_function)
                    .collect();

                if let Some(&idx) = struct_map.get(&imp.target_ty) {
                    types[idx].methods.extend(methods);
                } else if let Some(&idx) = enum_map.get(&imp.target_ty) {
                    enums[idx].methods.extend(methods);
                }
                // Methods on unknown types are silently dropped
            }
            DeclKind::Test(_) | DeclKind::Benchmark(_) | DeclKind::Export(_) | DeclKind::Package(_) | DeclKind::Union(_) => {}
        }
    }

    ModuleDescription {
        version: 1,
        module: module_name,
        file: file.to_string(),
        imports,
        types,
        enums,
        traits,
        functions,
        constants,
        externs,
    }
}

fn extract_struct(s: &StructDecl, opts: &DescribeOpts) -> StructDesc {
    let fields: Vec<FieldDesc> = s
        .fields
        .iter()
        .filter(|f| opts.show_all || f.is_pub)
        .map(|f| FieldDesc {
            name: f.name.clone(),
            type_str: f.ty.clone(),
            public: f.is_pub,
        })
        .collect();

    let methods: Vec<FunctionDesc> = s
        .methods
        .iter()
        .filter(|m| opts.show_all || m.is_pub)
        .map(extract_function)
        .collect();

    let type_params = extract_type_params(&s.type_params);
    let attrs = if s.attrs.is_empty() {
        None
    } else {
        Some(s.attrs.clone())
    };

    StructDesc {
        name: s.name.clone(),
        public: s.is_pub,
        type_params,
        attrs,
        fields,
        methods,
    }
}

fn extract_enum(e: &EnumDecl, opts: &DescribeOpts) -> EnumDesc {
    let variants: Vec<VariantDesc> = e
        .variants
        .iter()
        .map(|v| {
            let fields: Vec<FieldDesc> = v
                .fields
                .iter()
                .enumerate()
                .map(|(i, f)| FieldDesc {
                    // Positional fields use index as name
                    name: if f.name.is_empty() {
                        i.to_string()
                    } else {
                        f.name.clone()
                    },
                    type_str: f.ty.clone(),
                    public: true,
                })
                .collect();
            VariantDesc {
                name: v.name.clone(),
                fields,
            }
        })
        .collect();

    let methods: Vec<FunctionDesc> = e
        .methods
        .iter()
        .filter(|m| opts.show_all || m.is_pub)
        .map(extract_function)
        .collect();

    let type_params = extract_type_params(&e.type_params);

    EnumDesc {
        name: e.name.clone(),
        public: e.is_pub,
        type_params,
        variants,
        methods,
    }
}

fn extract_trait(t: &TraitDecl) -> TraitDesc {
    let methods: Vec<FunctionDesc> = t.methods.iter().map(extract_function).collect();

    TraitDesc {
        name: t.name.clone(),
        public: t.is_pub,
        methods,
    }
}

fn extract_function(f: &FnDecl) -> FunctionDesc {
    let mut self_mode = None;
    let mut params: Vec<ParamDesc> = Vec::new();

    for p in &f.params {
        if p.name == "self" {
            self_mode = Some(if p.is_take {
                "take".to_string()
            } else if p.is_mutate {
                "mutate".to_string()
            } else {
                "self".to_string()
            });
            continue;
        }

        let mode = if p.is_take {
            "take"
        } else if p.is_mutate {
            "mutate"
        } else {
            "borrow"
        };

        params.push(ParamDesc {
            name: p.name.clone(),
            type_str: p.ty.clone(),
            mode: mode.to_string(),
        });
    }

    let returns = parse_return_type(f.ret_ty.as_deref());

    let type_params = extract_type_params(&f.type_params);
    let attrs = if f.attrs.is_empty() {
        None
    } else {
        Some(f.attrs.clone())
    };

    let r#unsafe = if f.is_unsafe { Some(true) } else { None };
    let comptime = if f.is_comptime { Some(true) } else { None };

    FunctionDesc {
        name: f.name.clone(),
        public: f.is_pub,
        params,
        returns,
        self_mode,
        type_params,
        context: None, // Context clauses aren't in the AST signature yet
        attrs,
        r#unsafe,
        comptime,
    }
}

fn extract_extern(e: &ExternDecl) -> ExternDesc {
    let params: Vec<ParamDesc> = e
        .params
        .iter()
        .map(|p| {
            let mode = if p.is_take {
                "take"
            } else if p.is_mutate {
                "mutate"
            } else {
                "borrow"
            };
            ParamDesc {
                name: p.name.clone(),
                type_str: p.ty.clone(),
                mode: mode.to_string(),
            }
        })
        .collect();

    ExternDesc {
        abi: e.abi.clone(),
        name: e.name.clone(),
        params,
        returns: parse_return_type(e.ret_ty.as_deref()),
    }
}

/// Parse a return type string into ok/err components.
/// "T or E" → { ok: "T", err: "E" }
/// "Result<T, E>" → { ok: "T", err: "E" }  (parser normalizes "T or E" to this)
/// "T" → { ok: "T" }
/// None → { ok: "()" }
pub fn parse_return_type(ret_ty: Option<&str>) -> ReturnsDesc {
    match ret_ty {
        None => ReturnsDesc {
            ok: "()".to_string(),
            err: None,
        },
        Some(s) => {
            // Parser stores "T or E" as "Result<T, E>" — check both forms
            if let Some((ok, err)) = split_result_type(s) {
                ReturnsDesc {
                    ok: ok.trim().to_string(),
                    err: Some(err.trim().to_string()),
                }
            } else if let Some((ok, err)) = split_result_generic(s) {
                ReturnsDesc {
                    ok: ok.trim().to_string(),
                    err: Some(err.trim().to_string()),
                }
            } else {
                ReturnsDesc {
                    ok: s.trim().to_string(),
                    err: None,
                }
            }
        }
    }
}

/// Split "T or E" respecting angle bracket nesting.
fn split_result_type(s: &str) -> Option<(String, String)> {
    let mut depth = 0;
    let bytes = s.as_bytes();
    let or_pat = b" or ";

    for i in 0..bytes.len() {
        match bytes[i] {
            b'<' | b'(' => depth += 1,
            b'>' | b')' => depth -= 1,
            _ => {}
        }
        if depth == 0 && i + 4 <= bytes.len() && &bytes[i..i + 4] == or_pat {
            return Some((s[..i].to_string(), s[i + 4..].to_string()));
        }
    }
    None
}

/// Split "Result<T, E>" into (T, E).
fn split_result_generic(s: &str) -> Option<(String, String)> {
    let s = s.trim();
    if !s.starts_with("Result<") || !s.ends_with('>') {
        return None;
    }
    let inner = &s[7..s.len() - 1]; // Strip "Result<" and ">"
    // Split on ", " at depth 0
    let mut depth = 0;
    for (i, b) in inner.bytes().enumerate() {
        match b {
            b'<' | b'(' => depth += 1,
            b'>' | b')' => depth -= 1,
            b',' if depth == 0 => {
                let ok = inner[..i].trim();
                let err = inner[i + 1..].trim();
                return Some((ok.to_string(), err.to_string()));
            }
            _ => {}
        }
    }
    None
}

fn extract_type_params(tps: &[TypeParam]) -> Option<Vec<String>> {
    if tps.is_empty() {
        None
    } else {
        Some(tps.iter().map(|tp| tp.name.clone()).collect())
    }
}

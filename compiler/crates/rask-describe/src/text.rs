// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Human-readable text output for `rask describe`.

use crate::types::*;

/// Format a module description as compact text.
pub fn format_text(desc: &ModuleDescription) -> String {
    let mut out = String::new();

    out.push_str(&format!("{} ({})\n", desc.module, desc.file));

    for s in &desc.types {
        out.push('\n');
        format_struct(&mut out, s);
    }

    for e in &desc.enums {
        out.push('\n');
        format_enum(&mut out, e);
    }

    for t in &desc.traits {
        out.push('\n');
        format_trait(&mut out, t);
    }

    for f in &desc.functions {
        out.push('\n');
        out.push_str("  ");
        format_function(&mut out, f);
        out.push('\n');
    }

    for c in &desc.constants {
        out.push('\n');
        if c.public {
            out.push_str("  public ");
        } else {
            out.push_str("  ");
        }
        out.push_str(&format!("const {}", c.name));
        if let Some(ty) = &c.type_str {
            out.push_str(&format!(": {}", ty));
        }
        out.push('\n');
    }

    for e in &desc.externs {
        out.push('\n');
        out.push_str(&format!("  extern \"{}\" func {}(", e.abi, e.name));
        format_params(&mut out, &e.params);
        out.push(')');
        format_returns(&mut out, &e.returns);
        out.push('\n');
    }

    out
}

fn format_struct(out: &mut String, s: &StructDesc) {
    if s.public {
        out.push_str("  public struct ");
    } else {
        out.push_str("  struct ");
    }
    out.push_str(&s.name);
    format_type_params_inline(out, &s.type_params);
    out.push('\n');

    for f in &s.fields {
        out.push_str("    ");
        if f.public {
            out.push_str("public ");
        }
        out.push_str(&format!("{}: {}\n", f.name, f.type_str));
    }

    if !s.fields.is_empty() && !s.methods.is_empty() {
        out.push('\n');
    }

    for m in &s.methods {
        out.push_str("    ");
        format_function(out, m);
        out.push('\n');
    }
}

fn format_enum(out: &mut String, e: &EnumDesc) {
    if e.public {
        out.push_str("  public enum ");
    } else {
        out.push_str("  enum ");
    }
    out.push_str(&e.name);
    format_type_params_inline(out, &e.type_params);
    out.push('\n');

    for v in &e.variants {
        out.push_str(&format!("    {}", v.name));
        if !v.fields.is_empty() {
            out.push('(');
            let field_strs: Vec<String> = v
                .fields
                .iter()
                .map(|f| {
                    // Named fields: "name: type", positional: just "type"
                    if f.name.parse::<usize>().is_ok() {
                        f.type_str.clone()
                    } else {
                        format!("{}: {}", f.name, f.type_str)
                    }
                })
                .collect();
            out.push_str(&field_strs.join(", "));
            out.push(')');
        }
        out.push('\n');
    }

    if !e.variants.is_empty() && !e.methods.is_empty() {
        out.push('\n');
    }

    for m in &e.methods {
        out.push_str("    ");
        format_function(out, m);
        out.push('\n');
    }
}

fn format_trait(out: &mut String, t: &TraitDesc) {
    if t.public {
        out.push_str("  public trait ");
    } else {
        out.push_str("  trait ");
    }
    out.push_str(&t.name);
    out.push('\n');

    for m in &t.methods {
        out.push_str("    ");
        format_function(out, m);
        out.push('\n');
    }
}

fn format_function(out: &mut String, f: &FunctionDesc) {
    if f.public {
        out.push_str("public func ");
    } else {
        out.push_str("func ");
    }
    out.push_str(&f.name);
    format_type_params_inline(out, &f.type_params);
    out.push('(');

    let mut parts: Vec<String> = Vec::new();

    if let Some(sm) = &f.self_mode {
        match sm.as_str() {
            "self" => parts.push("self".to_string()),
            "read" => parts.push("read self".to_string()),
            "take" => parts.push("take self".to_string()),
            _ => parts.push(format!("{} self", sm)),
        }
    }

    for p in &f.params {
        let prefix = match p.mode.as_str() {
            "read" => "read ",
            "take" => "take ",
            _ => "",
        };
        parts.push(format!("{}{}: {}", prefix, p.name, p.type_str));
    }

    out.push_str(&parts.join(", "));
    out.push(')');

    format_returns(out, &f.returns);
}

fn format_returns(out: &mut String, ret: &ReturnsDesc) {
    if ret.ok == "()" && ret.err.is_none() {
        return;
    }
    out.push_str(" -> ");
    out.push_str(&ret.ok);
    if let Some(err) = &ret.err {
        out.push_str(" or ");
        out.push_str(err);
    }
}

fn format_params(out: &mut String, params: &[ParamDesc]) {
    let parts: Vec<String> = params
        .iter()
        .map(|p| {
            let prefix = match p.mode.as_str() {
                "read" => "read ",
                "take" => "take ",
                _ => "",
            };
            format!("{}{}: {}", prefix, p.name, p.type_str)
        })
        .collect();
    out.push_str(&parts.join(", "));
}

fn format_type_params_inline(out: &mut String, tps: &Option<Vec<String>>) {
    if let Some(params) = tps {
        if !params.is_empty() {
            out.push('<');
            out.push_str(&params.join(", "));
            out.push('>');
        }
    }
}

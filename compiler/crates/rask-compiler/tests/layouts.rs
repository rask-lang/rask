// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Layouts monomorphization computes from checked programs. Layout reads the
//! checker's type definitions, so these start from source.

use rask_compiler::{check_source, CfgConfig, CompilerConfig};

fn mono(src: &str) -> rask_mono::MonoProgram {
    let config = CompilerConfig::new(CfgConfig::from_host("debug", vec![]));
    let out = check_source("layouts.rk", src, &config);
    let checked = out.result.unwrap_or_else(|| panic!("didn't check: {:?}", out.diagnostics));
    rask_mono::monomorphize(&checked.typed, &checked.decls).expect("monomorphizes")
}

fn declared_enums(m: &rask_mono::MonoProgram) -> Vec<&str> {
    // `Ordering` is synthesized alongside the declared enums.
    m.enum_layouts.iter().map(|l| l.name.as_str()).filter(|n| *n != "Ordering").collect()
}

#[test]
fn struct_layouts_computed() {
    let m = mono("struct Point { x: i32, y: i32 }\nfunc main() {}\n");
    let names: Vec<&str> = m.struct_layouts.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, vec!["Point"]);
    assert_eq!(m.struct_layouts[0].size, 8);
}

#[test]
fn enum_layouts_computed() {
    let m = mono("enum Hue { Red, Green }\nfunc main() {}\n");
    assert_eq!(declared_enums(&m), vec!["Hue"]);
}

#[test]
fn struct_forward_references_enum() {
    // The struct comes before the enum it holds, so the enum's layout has to
    // be computed first for the field to get its real size.
    let m = mono(
        "struct Container { kind: Kind, value: i32 }\n\
         enum Kind { Alpha(x: i32, y: i32), Beta }\n\
         func main() {}\n",
    );
    assert_eq!(declared_enums(&m), vec!["Kind"]);
    let container = m.struct_layouts.iter().find(|l| l.name == "Container").unwrap();
    // Tag (1) + padding (3) + x (4) + y (4).
    let kind = container.fields.iter().find(|f| f.name == "kind").unwrap();
    assert_eq!(kind.size, 12);
}

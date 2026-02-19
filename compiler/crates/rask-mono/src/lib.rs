// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Monomorphization pass - eliminates generics by instantiating concrete copies.
//!
//! Takes type-checked AST and produces monomorphized program with:
//! - Concrete function instances for each unique (function_id, [type_args])
//! - Computed memory layouts for all structs and enums
//! - Reachability analysis starting from main()

mod instantiate;
mod layout;
mod reachability;

pub use instantiate::instantiate_function;
pub use layout::{
    compute_enum_layout, compute_struct_layout, compute_union_layout, type_size_align, EnumLayout, FieldLayout,
    LayoutCache, StructLayout, VariantLayout,
};
pub use reachability::Monomorphizer;

use rask_ast::decl::{Decl, DeclKind};
use rask_types::{Type, TypedProgram};

/// Monomorphized program with all generics eliminated
pub struct MonoProgram {
    pub functions: Vec<MonoFunction>,
    pub struct_layouts: Vec<StructLayout>,
    pub enum_layouts: Vec<EnumLayout>,
}

/// Monomorphized function instance
pub struct MonoFunction {
    pub name: String,
    pub type_args: Vec<Type>,
    pub body: Decl,
}

/// Monomorphize a type-checked program.
///
/// Architecture: reachability drives instantiation (tree-shaking).
/// Only functions reachable from main() get instantiated.
///
/// 1. Build function lookup table from declarations
/// 2. BFS from main(): discover calls → instantiate on demand → walk instantiated body
/// 3. Compute layouts for all referenced structs/enums
pub fn monomorphize(
    program: &TypedProgram,
    decls: &[Decl],
) -> Result<MonoProgram, MonomorphizeError> {
    let mut mono = Monomorphizer::new(decls, &program.call_type_args);

    if !mono.add_entry("main") {
        return Err(MonomorphizeError::NoEntryPoint);
    }

    mono.run();

    // Compute layouts for concrete (non-generic) struct/enum types.
    // Generic types need concrete type_args for correct field sizes;
    // their layouts are computed per-instantiation, not here.
    // Layout cache lets structs reference other user-defined types.
    let mut layout_cache = LayoutCache::new();
    let mut struct_layouts = Vec::new();
    let mut enum_layouts = Vec::new();
    for decl in decls {
        match &decl.kind {
            DeclKind::Struct(s) if s.type_params.is_empty() => {
                let layout = compute_struct_layout(decl, &[], &layout_cache);
                layout_cache.insert(s.name.clone(), (layout.size, layout.align));
                struct_layouts.push(layout);
            }
            DeclKind::Enum(e) if e.type_params.is_empty() => {
                let layout = compute_enum_layout(decl, &[], &layout_cache);
                layout_cache.insert(e.name.clone(), (layout.size, layout.align));
                enum_layouts.push(layout);
            }
            DeclKind::Union(u) => {
                let layout = compute_union_layout(decl, &layout_cache);
                layout_cache.insert(u.name.clone(), (layout.size, layout.align));
                struct_layouts.push(layout);
            }
            _ => {}
        }
    }

    Ok(MonoProgram {
        functions: mono.results,
        struct_layouts,
        enum_layouts,
    })
}

#[derive(Debug)]
pub enum MonomorphizeError {
    NoEntryPoint,
    UnresolvedGeneric {
        function_name: String,
        type_param: String,
    },
    LayoutError {
        type_name: String,
        reason: String,
    },
}

// ─── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use rask_ast::decl::{
        Decl, DeclKind, EnumDecl, FnDecl, ImplDecl, Param, StructDecl, TypeParam, Variant, Field,
    };
    use rask_ast::expr::{ArgMode, CallArg, Expr, ExprKind};
    use rask_ast::stmt::{Stmt, StmtKind};
    use rask_ast::{NodeId, Span};

    fn sp() -> Span {
        Span::new(0, 0)
    }

    fn int_expr(val: i64) -> Expr {
        Expr {
            id: NodeId(100),
            kind: ExprKind::Int(val, None),
            span: sp(),
        }
    }

    fn ident_expr(name: &str) -> Expr {
        Expr {
            id: NodeId(101),
            kind: ExprKind::Ident(name.to_string()),
            span: sp(),
        }
    }

    fn call_expr(func_name: &str, args: Vec<Expr>) -> Expr {
        Expr {
            id: NodeId(102),
            kind: ExprKind::Call {
                func: Box::new(ident_expr(func_name)),
                args: args.into_iter().map(|expr| CallArg { mode: ArgMode::Default, expr }).collect(),
            },
            span: sp(),
        }
    }

    fn return_stmt(val: Option<Expr>) -> Stmt {
        Stmt {
            id: NodeId(200),
            kind: StmtKind::Return(val),
            span: sp(),
        }
    }

    fn expr_stmt(e: Expr) -> Stmt {
        Stmt {
            id: NodeId(201),
            kind: StmtKind::Expr(e),
            span: sp(),
        }
    }

    fn make_fn(name: &str, params: Vec<(&str, &str)>, ret_ty: Option<&str>, body: Vec<Stmt>) -> Decl {
        Decl {
            id: NodeId(0),
            kind: DeclKind::Fn(FnDecl {
                name: name.to_string(),
                type_params: vec![],
                params: params
                    .into_iter()
                    .map(|(n, ty)| Param {
                        name: n.to_string(),
                        name_span: sp(),
                        ty: ty.to_string(),
                        is_take: false,
                        is_mutate: false,
                        default: None,
                    })
                    .collect(),
                ret_ty: ret_ty.map(|s| s.to_string()),
                context_clauses: vec![],
                body,
                is_pub: false,
                is_comptime: false,
                is_unsafe: false,
                abi: None,
                attrs: vec![],
                doc: None,
                span: sp(),
            }),
            span: sp(),
        }
    }

    fn make_generic_fn(
        name: &str,
        type_params: Vec<&str>,
        params: Vec<(&str, &str)>,
        ret_ty: Option<&str>,
        body: Vec<Stmt>,
    ) -> Decl {
        Decl {
            id: NodeId(0),
            kind: DeclKind::Fn(FnDecl {
                name: name.to_string(),
                type_params: type_params
                    .into_iter()
                    .map(|tp| TypeParam {
                        name: tp.to_string(),
                        is_comptime: false,
                        comptime_type: None,
                        bounds: vec![],
                    })
                    .collect(),
                params: params
                    .into_iter()
                    .map(|(n, ty)| Param {
                        name: n.to_string(),
                        name_span: sp(),
                        ty: ty.to_string(),
                        is_take: false,
                        is_mutate: false,
                        default: None,
                    })
                    .collect(),
                ret_ty: ret_ty.map(|s| s.to_string()),
                context_clauses: vec![],
                body,
                is_pub: false,
                is_comptime: false,
                is_unsafe: false,
                abi: None,
                attrs: vec![],
                doc: None,
                span: sp(),
            }),
            span: sp(),
        }
    }

    fn dummy_typed_program() -> TypedProgram {
        TypedProgram {
            symbols: rask_resolve::SymbolTable::new(),
            resolutions: std::collections::HashMap::new(),
            types: rask_types::TypeTable::new(),
            node_types: std::collections::HashMap::new(),
            call_type_args: std::collections::HashMap::new(),
        }
    }

    // ── Monomorphize entry point ────────────────────────────────

    #[test]
    fn no_main_returns_error() {
        let decls = vec![make_fn("helper", vec![], None, vec![return_stmt(None)])];
        let tp = dummy_typed_program();
        let result = monomorphize(&tp, &decls);
        assert!(matches!(result, Err(MonomorphizeError::NoEntryPoint)));
    }

    #[test]
    fn main_only() {
        let decls = vec![make_fn(
            "main",
            vec![],
            None,
            vec![return_stmt(None)],
        )];
        let tp = dummy_typed_program();
        let result = monomorphize(&tp, &decls).unwrap();
        assert_eq!(result.functions.len(), 1);
        assert_eq!(result.functions[0].name, "main");
    }

    #[test]
    fn main_calls_helper() {
        let decls = vec![
            make_fn(
                "main",
                vec![],
                None,
                vec![expr_stmt(call_expr("helper", vec![])), return_stmt(None)],
            ),
            make_fn("helper", vec![], None, vec![return_stmt(None)]),
        ];
        let tp = dummy_typed_program();
        let result = monomorphize(&tp, &decls).unwrap();
        assert_eq!(result.functions.len(), 2);
        let names: Vec<&str> = result.functions.iter().map(|f| f.name.as_str()).collect();
        assert!(names.contains(&"main"));
        assert!(names.contains(&"helper"));
    }

    #[test]
    fn unreachable_function_excluded() {
        let decls = vec![
            make_fn("main", vec![], None, vec![return_stmt(None)]),
            make_fn("dead_code", vec![], None, vec![return_stmt(None)]),
        ];
        let tp = dummy_typed_program();
        let result = monomorphize(&tp, &decls).unwrap();
        assert_eq!(result.functions.len(), 1);
        assert_eq!(result.functions[0].name, "main");
    }

    #[test]
    fn transitive_calls() {
        // main → a → b → c
        let decls = vec![
            make_fn(
                "main",
                vec![],
                None,
                vec![expr_stmt(call_expr("a", vec![])), return_stmt(None)],
            ),
            make_fn(
                "a",
                vec![],
                None,
                vec![expr_stmt(call_expr("b", vec![])), return_stmt(None)],
            ),
            make_fn(
                "b",
                vec![],
                None,
                vec![expr_stmt(call_expr("c", vec![])), return_stmt(None)],
            ),
            make_fn("c", vec![], None, vec![return_stmt(None)]),
        ];
        let tp = dummy_typed_program();
        let result = monomorphize(&tp, &decls).unwrap();
        assert_eq!(result.functions.len(), 4);
    }

    #[test]
    fn recursive_function_terminates() {
        // main calls itself (cycle)
        let decls = vec![make_fn(
            "main",
            vec![],
            None,
            vec![expr_stmt(call_expr("main", vec![])), return_stmt(None)],
        )];
        let tp = dummy_typed_program();
        let result = monomorphize(&tp, &decls).unwrap();
        assert_eq!(result.functions.len(), 1);
    }

    #[test]
    fn mutual_recursion_terminates() {
        // a → b → a (cycle)
        let decls = vec![
            make_fn(
                "main",
                vec![],
                None,
                vec![expr_stmt(call_expr("a", vec![])), return_stmt(None)],
            ),
            make_fn(
                "a",
                vec![],
                None,
                vec![expr_stmt(call_expr("b", vec![])), return_stmt(None)],
            ),
            make_fn(
                "b",
                vec![],
                None,
                vec![expr_stmt(call_expr("a", vec![])), return_stmt(None)],
            ),
        ];
        let tp = dummy_typed_program();
        let result = monomorphize(&tp, &decls).unwrap();
        assert_eq!(result.functions.len(), 3);
    }

    #[test]
    fn struct_layouts_computed() {
        let decls = vec![
            make_fn("main", vec![], None, vec![return_stmt(None)]),
            Decl {
                id: NodeId(0),
                kind: DeclKind::Struct(StructDecl {
                    name: "Point".to_string(),
                    type_params: vec![],
                    fields: vec![
                        Field { name: "x".to_string(), name_span: sp(), ty: "i32".to_string(), is_pub: false },
                        Field { name: "y".to_string(), name_span: sp(), ty: "i32".to_string(), is_pub: false },
                    ],
                    methods: vec![],
                    is_pub: false,
                    attrs: vec![],
                    doc: None,
                }),
                span: sp(),
            },
        ];
        let tp = dummy_typed_program();
        let result = monomorphize(&tp, &decls).unwrap();
        assert_eq!(result.struct_layouts.len(), 1);
        assert_eq!(result.struct_layouts[0].name, "Point");
    }

    #[test]
    fn enum_layouts_computed() {
        let decls = vec![
            make_fn("main", vec![], None, vec![return_stmt(None)]),
            Decl {
                id: NodeId(0),
                kind: DeclKind::Enum(EnumDecl {
                    name: "Color".to_string(),
                    type_params: vec![],
                    variants: vec![
                        Variant { name: "Red".to_string(), fields: vec![] },
                        Variant { name: "Green".to_string(), fields: vec![] },
                    ],
                    methods: vec![],
                    is_pub: false,
                    doc: None,
                }),
                span: sp(),
            },
        ];
        let tp = dummy_typed_program();
        let result = monomorphize(&tp, &decls).unwrap();
        assert_eq!(result.enum_layouts.len(), 1);
        assert_eq!(result.enum_layouts[0].name, "Color");
    }

    // ── Instantiation ───────────────────────────────────────────

    #[test]
    fn instantiate_removes_type_params() {
        let decl = make_generic_fn(
            "identity",
            vec!["T"],
            vec![("x", "T")],
            Some("T"),
            vec![return_stmt(Some(ident_expr("x")))],
        );
        let result = instantiate_function(&decl, &[Type::I32]);
        if let DeclKind::Fn(f) = &result.kind {
            assert!(f.type_params.is_empty());
            assert_eq!(f.params[0].ty, "i32"); // substituted
        } else {
            panic!("Expected function declaration");
        }
    }

    #[test]
    fn instantiate_preserves_body() {
        let decl = make_generic_fn(
            "identity",
            vec!["T"],
            vec![("x", "T")],
            Some("T"),
            vec![return_stmt(Some(ident_expr("x")))],
        );
        let result = instantiate_function(&decl, &[Type::I64]);
        if let DeclKind::Fn(f) = &result.kind {
            assert_eq!(f.body.len(), 1);
            assert!(matches!(f.body[0].kind, StmtKind::Return(Some(_))));
        } else {
            panic!("Expected function declaration");
        }
    }

    #[test]
    fn instantiate_fresh_node_ids() {
        // Use a distinct NodeId for the original so we can verify the clone gets a different one
        let mut decl = make_generic_fn(
            "id",
            vec!["T"],
            vec![("x", "T")],
            None,
            vec![return_stmt(Some(ident_expr("x")))],
        );
        decl.id = NodeId(9999);
        let result = instantiate_function(&decl, &[Type::Bool]);
        // Substitutor generates sequential IDs starting at 0, so result.id != 9999
        assert_ne!(result.id, decl.id);
    }

    // ── Reachability walker ─────────────────────────────────────

    #[test]
    fn reachability_discovers_nested_calls() {
        // main → { let x = foo(1); bar(x) }
        let decls = vec![
            make_fn(
                "main",
                vec![],
                None,
                vec![
                    Stmt {
                        id: NodeId(10),
                        kind: StmtKind::Const {
                            name: "x".to_string(),
                            name_span: sp(),
                            ty: None,
                            init: call_expr("foo", vec![int_expr(1)]),
                        },
                        span: sp(),
                    },
                    expr_stmt(call_expr("bar", vec![ident_expr("x")])),
                    return_stmt(None),
                ],
            ),
            make_fn("foo", vec![("n", "i32")], Some("i32"), vec![return_stmt(Some(ident_expr("n")))]),
            make_fn("bar", vec![("n", "i32")], None, vec![return_stmt(None)]),
            make_fn("unused", vec![], None, vec![return_stmt(None)]),
        ];

        let empty_type_args = std::collections::HashMap::new();
        let mut mono = Monomorphizer::new(&decls, &empty_type_args);
        assert!(mono.add_entry("main"));
        mono.run();

        let names: Vec<&str> = mono.results.iter().map(|f| f.name.as_str()).collect();
        assert!(names.contains(&"main"));
        assert!(names.contains(&"foo"));
        assert!(names.contains(&"bar"));
        assert!(!names.contains(&"unused"));
    }

    #[test]
    fn reachability_handles_conditionals() {
        // main → if true { a() } else { b() }
        let decls = vec![
            make_fn(
                "main",
                vec![],
                None,
                vec![expr_stmt(Expr {
                    id: NodeId(50),
                    kind: ExprKind::If {
                        cond: Box::new(Expr {
                            id: NodeId(51),
                            kind: ExprKind::Bool(true),
                            span: sp(),
                        }),
                        then_branch: Box::new(call_expr("a", vec![])),
                        else_branch: Some(Box::new(call_expr("b", vec![]))),
                    },
                    span: sp(),
                })],
            ),
            make_fn("a", vec![], None, vec![return_stmt(None)]),
            make_fn("b", vec![], None, vec![return_stmt(None)]),
        ];

        let empty_type_args = std::collections::HashMap::new();
        let mut mono = Monomorphizer::new(&decls, &empty_type_args);
        mono.add_entry("main");
        mono.run();

        // Both branches are conservatively included
        let names: Vec<&str> = mono.results.iter().map(|f| f.name.as_str()).collect();
        assert!(names.contains(&"a"));
        assert!(names.contains(&"b"));
    }

    // ── Method reachability ─────────────────────────────────────

    fn method_call_expr(object: Expr, method: &str, args: Vec<Expr>) -> Expr {
        Expr {
            id: NodeId(300),
            kind: ExprKind::MethodCall {
                object: Box::new(object),
                method: method.to_string(),
                type_args: None,
                args: args.into_iter().map(|expr| CallArg { mode: ArgMode::Default, expr }).collect(),
            },
            span: sp(),
        }
    }

    fn make_method(name: &str, params: Vec<(&str, &str)>, ret_ty: Option<&str>, body: Vec<Stmt>) -> FnDecl {
        FnDecl {
            name: name.to_string(),
            type_params: vec![],
            params: params
                .into_iter()
                .map(|(n, ty)| Param {
                    name: n.to_string(),
                    name_span: sp(),
                    ty: ty.to_string(),
                    is_take: false,
                    is_mutate: false,
                    default: None,
                })
                .collect(),
            ret_ty: ret_ty.map(|s| s.to_string()),
            context_clauses: vec![],
            body,
            is_pub: false,
            is_comptime: false,
            is_unsafe: false,
            abi: None,
            attrs: vec![],
            doc: None,
            span: sp(),
        }
    }

    #[test]
    fn method_call_on_type_enqueues_static_method() {
        // main calls Point.new() — static method on struct
        let decls = vec![
            make_fn(
                "main",
                vec![],
                None,
                vec![
                    expr_stmt(method_call_expr(ident_expr("Point"), "new", vec![])),
                    return_stmt(None),
                ],
            ),
            Decl {
                id: NodeId(0),
                kind: DeclKind::Struct(StructDecl {
                    name: "Point".to_string(),
                    type_params: vec![],
                    fields: vec![
                        Field { name: "x".to_string(), name_span: sp(), ty: "i32".to_string(), is_pub: false },
                        Field { name: "y".to_string(), name_span: sp(), ty: "i32".to_string(), is_pub: false },
                    ],
                    methods: vec![
                        make_method("new", vec![], Some("Point"), vec![return_stmt(None)]),
                    ],
                    is_pub: false,
                    attrs: vec![],
                    doc: None,
                }),
                span: sp(),
            },
        ];

        let empty_type_args = std::collections::HashMap::new();
        let mut mono = Monomorphizer::new(&decls, &empty_type_args);
        mono.add_entry("main");
        mono.run();

        let names: Vec<&str> = mono.results.iter().map(|f| f.name.as_str()).collect();
        assert!(names.contains(&"main"));
        assert!(names.contains(&"Point_new"), "static method should be reachable: {:?}", names);
    }

    #[test]
    fn method_call_on_value_enqueues_instance_method() {
        // main calls p.distance() — instance method via bare name
        let decls = vec![
            make_fn(
                "main",
                vec![],
                None,
                vec![
                    expr_stmt(method_call_expr(ident_expr("p"), "distance", vec![])),
                    return_stmt(None),
                ],
            ),
            Decl {
                id: NodeId(0),
                kind: DeclKind::Impl(ImplDecl {
                    trait_name: None,
                    target_ty: "Point".to_string(),
                    methods: vec![
                        make_method("distance", vec![("self", "Point")], Some("f64"), vec![return_stmt(None)]),
                    ],
                    doc: None,
                    is_unsafe: false,
                }),
                span: sp(),
            },
        ];

        let empty_type_args = std::collections::HashMap::new();
        let mut mono = Monomorphizer::new(&decls, &empty_type_args);
        mono.add_entry("main");
        mono.run();

        let names: Vec<&str> = mono.results.iter().map(|f| f.name.as_str()).collect();
        assert!(names.contains(&"main"));
        // Instance call on "p" enqueues via bare name → Point_distance (from method_by_bare_name)
        assert!(names.contains(&"Point_distance"), "instance method should be reachable: {:?}", names);
    }

    #[test]
    fn method_in_impl_block_reachable() {
        // main calls Counter.increment() via extend block
        let decls = vec![
            make_fn(
                "main",
                vec![],
                None,
                vec![
                    expr_stmt(method_call_expr(ident_expr("Counter"), "increment", vec![])),
                    return_stmt(None),
                ],
            ),
            Decl {
                id: NodeId(0),
                kind: DeclKind::Impl(ImplDecl {
                    trait_name: None,
                    target_ty: "Counter".to_string(),
                    methods: vec![
                        make_method("increment", vec![("self", "Counter")], None, vec![return_stmt(None)]),
                    ],
                    doc: None,
                    is_unsafe: false,
                }),
                span: sp(),
            },
        ];

        let empty_type_args = std::collections::HashMap::new();
        let mut mono = Monomorphizer::new(&decls, &empty_type_args);
        mono.add_entry("main");
        mono.run();

        let names: Vec<&str> = mono.results.iter().map(|f| f.name.as_str()).collect();
        assert!(names.contains(&"Counter_increment"), "impl method should be reachable: {:?}", names);
    }

    #[test]
    fn unreachable_method_excluded() {
        // main doesn't call any methods — dead_method should be excluded
        let decls = vec![
            make_fn("main", vec![], None, vec![return_stmt(None)]),
            Decl {
                id: NodeId(0),
                kind: DeclKind::Struct(StructDecl {
                    name: "Widget".to_string(),
                    type_params: vec![],
                    fields: vec![],
                    methods: vec![
                        make_method("dead_method", vec![], None, vec![return_stmt(None)]),
                    ],
                    is_pub: false,
                    attrs: vec![],
                    doc: None,
                }),
                span: sp(),
            },
        ];

        let empty_type_args = std::collections::HashMap::new();
        let mut mono = Monomorphizer::new(&decls, &empty_type_args);
        mono.add_entry("main");
        mono.run();

        assert_eq!(mono.results.len(), 1);
        assert_eq!(mono.results[0].name, "main");
    }

    #[test]
    fn method_body_transitively_discovers_calls() {
        // main → Point.new() → helper() (transitive through method body)
        let decls = vec![
            make_fn(
                "main",
                vec![],
                None,
                vec![
                    expr_stmt(method_call_expr(ident_expr("Point"), "new", vec![])),
                    return_stmt(None),
                ],
            ),
            Decl {
                id: NodeId(0),
                kind: DeclKind::Struct(StructDecl {
                    name: "Point".to_string(),
                    type_params: vec![],
                    fields: vec![],
                    methods: vec![
                        make_method("new", vec![], Some("Point"), vec![
                            expr_stmt(call_expr("helper", vec![])),
                            return_stmt(None),
                        ]),
                    ],
                    is_pub: false,
                    attrs: vec![],
                    doc: None,
                }),
                span: sp(),
            },
            make_fn("helper", vec![], None, vec![return_stmt(None)]),
            make_fn("unused", vec![], None, vec![return_stmt(None)]),
        ];

        let empty_type_args = std::collections::HashMap::new();
        let mut mono = Monomorphizer::new(&decls, &empty_type_args);
        mono.add_entry("main");
        mono.run();

        let names: Vec<&str> = mono.results.iter().map(|f| f.name.as_str()).collect();
        assert!(names.contains(&"main"));
        assert!(names.contains(&"Point_new"));
        assert!(names.contains(&"helper"), "transitive call from method body should be discovered");
        assert!(!names.contains(&"unused"));
    }

    #[test]
    fn enum_method_reachable() {
        // main calls Color.default()
        let decls = vec![
            make_fn(
                "main",
                vec![],
                None,
                vec![
                    expr_stmt(method_call_expr(ident_expr("Color"), "default", vec![])),
                    return_stmt(None),
                ],
            ),
            Decl {
                id: NodeId(0),
                kind: DeclKind::Enum(EnumDecl {
                    name: "Color".to_string(),
                    type_params: vec![],
                    variants: vec![
                        Variant { name: "Red".to_string(), fields: vec![] },
                        Variant { name: "Blue".to_string(), fields: vec![] },
                    ],
                    methods: vec![
                        make_method("default", vec![], Some("Color"), vec![return_stmt(None)]),
                    ],
                    is_pub: false,
                    doc: None,
                }),
                span: sp(),
            },
        ];

        let empty_type_args = std::collections::HashMap::new();
        let mut mono = Monomorphizer::new(&decls, &empty_type_args);
        mono.add_entry("main");
        mono.run();

        let names: Vec<&str> = mono.results.iter().map(|f| f.name.as_str()).collect();
        assert!(names.contains(&"Color_default"), "enum method should be reachable: {:?}", names);
    }
}

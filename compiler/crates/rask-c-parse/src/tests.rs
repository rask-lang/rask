// SPDX-License-Identifier: (MIT OR Apache-2.0)

use crate::*;
use crate::translate;

    fn parse(src: &str) -> CParseResult {
        parse_c_header(src).expect("parse failed")
    }

    // ---- Lexer + Parser integration ----

    #[test]
    fn parse_function_decl() {
        let r = parse("int printf(const char *fmt, ...);");
        assert_eq!(r.decls.len(), 1);
        match &r.decls[0] {
            CDecl::Function(f) => {
                assert_eq!(f.name, "printf");
                assert!(f.is_variadic);
                assert_eq!(f.ret_ty, CType::Int);
                assert_eq!(f.params.len(), 1);
            }
            other => panic!("expected Function, got {:?}", other),
        }
    }

    #[test]
    fn parse_void_function() {
        let r = parse("void exit(int status);");
        match &r.decls[0] {
            CDecl::Function(f) => {
                assert_eq!(f.name, "exit");
                assert_eq!(f.ret_ty, CType::Void);
                assert!(!f.is_variadic);
            }
            other => panic!("expected Function, got {:?}", other),
        }
    }

    #[test]
    fn parse_struct_with_fields() {
        let r = parse("struct Point { int x; int y; };");
        let found_struct = r.decls.iter().find(|d| matches!(d, CDecl::Struct(_)));
        match found_struct {
            Some(CDecl::Struct(s)) => {
                assert_eq!(s.tag.as_deref(), Some("Point"));
                assert_eq!(s.fields.len(), 2);
                assert_eq!(s.fields[0].name, "x");
                assert_eq!(s.fields[1].name, "y");
                assert!(!s.is_forward);
            }
            other => panic!("expected Struct, got {:?}", other),
        }
    }

    #[test]
    fn parse_forward_decl() {
        let r = parse("struct Opaque;");
        match &r.decls[0] {
            CDecl::Struct(s) => {
                assert_eq!(s.tag.as_deref(), Some("Opaque"));
                assert!(s.is_forward);
            }
            other => panic!("expected forward Struct, got {:?}", other),
        }
    }

    #[test]
    fn parse_enum() {
        let r = parse("enum Color { RED, GREEN = 5, BLUE };");
        let found_enum = r.decls.iter().find(|d| matches!(d, CDecl::Enum(_)));
        match found_enum {
            Some(CDecl::Enum(e)) => {
                assert_eq!(e.tag.as_deref(), Some("Color"));
                assert_eq!(e.variants.len(), 3);
                assert_eq!(e.variants[0].name, "RED");
                assert_eq!(e.variants[0].value, Some(0));
                assert_eq!(e.variants[1].name, "GREEN");
                assert_eq!(e.variants[1].value, Some(5));
                assert_eq!(e.variants[2].name, "BLUE");
                assert_eq!(e.variants[2].value, Some(6));
            }
            other => panic!("expected Enum, got {:?}", other),
        }
    }

    #[test]
    fn parse_typedef_struct() {
        let r = parse("typedef struct { int fd; } FileHandle;");
        let has_struct = r.decls.iter().any(|d| matches!(d, CDecl::Struct(_)));
        let has_typedef = r.decls.iter().any(|d| matches!(d, CDecl::Typedef(_)));
        assert!(has_struct, "should emit struct");
        assert!(has_typedef, "should emit typedef");
    }

    #[test]
    fn parse_define_integer() {
        let r = parse("#define MAX_SIZE 1024\n");
        match &r.decls[0] {
            CDecl::Define(d) => {
                assert_eq!(d.name, "MAX_SIZE");
                assert_eq!(d.kind, CDefineKind::Integer(1024));
            }
            other => panic!("expected Define, got {:?}", other),
        }
    }

    #[test]
    fn parse_define_string() {
        let r = parse("#define VERSION \"1.0\"\n");
        match &r.decls[0] {
            CDecl::Define(d) => {
                assert_eq!(d.name, "VERSION");
                assert_eq!(d.kind, CDefineKind::String("1.0".into()));
            }
            other => panic!("expected Define, got {:?}", other),
        }
    }

    #[test]
    fn parse_define_function_macro() {
        let r = parse("#define MAX(a, b) ((a) > (b) ? (a) : (b))\n");
        match &r.decls[0] {
            CDecl::Define(d) => {
                assert_eq!(d.name, "MAX");
                assert!(matches!(d.kind, CDefineKind::FunctionMacro { .. }));
            }
            other => panic!("expected Define, got {:?}", other),
        }
    }

    #[test]
    fn parse_typedef_pointer() {
        let r = parse("typedef void *HANDLE;");
        match &r.decls[0] {
            CDecl::Typedef(td) => {
                assert_eq!(td.name, "HANDLE");
                assert_eq!(td.target, CType::Pointer(Box::new(CType::Void)));
            }
            other => panic!("expected Typedef, got {:?}", other),
        }
    }

    #[test]
    fn parse_unsigned_long_long() {
        let r = parse("unsigned long long get_size(void);");
        match &r.decls[0] {
            CDecl::Function(f) => {
                assert_eq!(f.ret_ty, CType::UnsignedLongLong);
                assert!(f.params.is_empty());
            }
            other => panic!("expected Function, got {:?}", other),
        }
    }

    #[test]
    fn parse_stdint_types() {
        let r = parse("uint32_t hash(const uint8_t *data, size_t len);");
        match &r.decls[0] {
            CDecl::Function(f) => {
                assert_eq!(f.ret_ty, CType::FixedInt { bits: 32, signed: false });
                assert_eq!(f.params.len(), 2);
            }
            other => panic!("expected Function, got {:?}", other),
        }
    }

    #[test]
    fn skip_static_function() {
        let r = parse("static int helper(void) { return 0; }");
        // Static functions should be skipped (per spec: internal linkage)
        assert!(r.decls.is_empty(), "static functions should be skipped");
    }

    #[test]
    fn skip_inline_body() {
        let r = parse("inline int square(int x) { return x * x; }");
        // Inline functions with bodies — kept as declaration, body discarded.
        // (inline is not static, so it's imported)
        match &r.decls[0] {
            CDecl::Function(f) => {
                assert_eq!(f.name, "square");
                assert!(f.is_inline);
            }
            other => panic!("expected Function, got {:?}", other),
        }
    }

    #[test]
    fn comments_stripped() {
        let r = parse("/* block */ int foo(void); // line comment\n");
        match &r.decls[0] {
            CDecl::Function(f) => assert_eq!(f.name, "foo"),
            other => panic!("expected Function, got {:?}", other),
        }
    }

    #[test]
    fn parse_function_pointer_param() {
        let r = parse("void qsort(void *base, size_t n, size_t size, int (*cmp)(const void *, const void *));");
        match &r.decls[0] {
            CDecl::Function(f) => {
                assert_eq!(f.name, "qsort");
                assert_eq!(f.params.len(), 4);
                // Fourth param should be a function pointer
                match &f.params[3].ty {
                    CType::FuncPtr { ret, params, .. } => {
                        assert_eq!(**ret, CType::Int);
                        assert_eq!(params.len(), 2);
                    }
                    other => panic!("expected FuncPtr param, got {:?}", other),
                }
            }
            other => panic!("expected Function, got {:?}", other),
        }
    }

    #[test]
    fn skip_preprocessor_conditionals() {
        let src = r#"
#ifdef _WIN32
int win_only(void);
#else
int unix_only(void);
#endif
"#;
        // Both branches visible (no real preprocessing — just skips directives)
        let r = parse(src);
        let funcs: Vec<_> = r.decls.iter().filter(|d| matches!(d, CDecl::Function(_))).collect();
        assert_eq!(funcs.len(), 2);
    }

    #[test]
    fn parse_extern_variable() {
        let r = parse("extern int errno;");
        match &r.decls[0] {
            CDecl::Variable(v) => {
                assert_eq!(v.name, "errno");
                assert!(v.is_extern);
                assert_eq!(v.ty, CType::Int);
            }
            other => panic!("expected Variable, got {:?}", other),
        }
    }

    // ---- Translator tests ----

    #[test]
    fn translate_function() {
        let r = parse("int open(const char *path, int flags);");
        let result = translate::translate(&r, &[]);
        match &result.decls[0] {
            translate::RaskCDecl::Function(f) => {
                assert_eq!(f.name, "open");
                assert_eq!(f.ret_ty, "c_int");
                assert_eq!(f.params[0].ty, "*u8");
                assert_eq!(f.params[1].ty, "c_int");
            }
            other => panic!("expected Function, got {:?}", other),
        }
    }

    #[test]
    fn translate_struct() {
        let r = parse("struct Point { int x; float y; };");
        let result = translate::translate(&r, &[]);
        let found = result.decls.iter().find(|d| matches!(d, translate::RaskCDecl::Struct(_)));
        match found {
            Some(translate::RaskCDecl::Struct(s)) => {
                assert_eq!(s.name, "Point");
                assert_eq!(s.fields[0].ty, "c_int");
                assert_eq!(s.fields[1].ty, "f32");
            }
            other => panic!("expected Struct, got {:?}", other),
        }
    }

    #[test]
    fn translate_opaque() {
        let r = parse("struct sqlite3;");
        let result = translate::translate(&r, &[]);
        match &result.decls[0] {
            translate::RaskCDecl::Struct(s) => {
                assert_eq!(s.name, "sqlite3");
                assert!(s.is_opaque);
            }
            other => panic!("expected opaque Struct, got {:?}", other),
        }
    }

    #[test]
    fn translate_hiding() {
        let r = parse("int keep(void);\nint hide(void);");
        let result = translate::translate(&r, &["hide".to_string()]);
        assert_eq!(result.decls.len(), 1);
        match &result.decls[0] {
            translate::RaskCDecl::Function(f) => assert_eq!(f.name, "keep"),
            other => panic!("expected Function, got {:?}", other),
        }
    }

    #[test]
    fn translate_define_to_const() {
        let r = parse("#define SQLITE_OK 0\n");
        let result = translate::translate(&r, &[]);
        match &result.decls[0] {
            translate::RaskCDecl::Const(c) => {
                assert_eq!(c.name, "SQLITE_OK");
                assert_eq!(c.ty, "c_int");
                assert_eq!(c.value_repr, "0");
            }
            other => panic!("expected Const, got {:?}", other),
        }
    }

    #[test]
    fn translate_size_t() {
        let r = parse("size_t strlen(const char *s);");
        let result = translate::translate(&r, &[]);
        match &result.decls[0] {
            translate::RaskCDecl::Function(f) => {
                assert_eq!(f.ret_ty, "c_size");
            }
            other => panic!("expected Function, got {:?}", other),
        }
    }

    #[test]
    fn translate_void_return() {
        let r = parse("void free(void *ptr);");
        let result = translate::translate(&r, &[]);
        match &result.decls[0] {
            translate::RaskCDecl::Function(f) => {
                assert_eq!(f.ret_ty, "");
                assert_eq!(f.params[0].ty, "*void");
            }
            other => panic!("expected Function, got {:?}", other),
        }
    }

    #[test]
    fn render_rask_output() {
        let r = parse("int add(int a, int b);");
        let result = translate::translate(&r, &[]);
        let output = translate::render_rask(&result);
        assert!(output.contains("extern \"C\" func add("));
        assert!(output.contains("c_int"));
    }

    // ---- Resilience ----

    #[test]
    fn recovers_from_unparseable() {
        let src = r#"
int good_func(void);
this is garbage that should not parse;
int another_good(int x);
"#;
        let r = parse(src);
        let funcs: Vec<_> = r.decls.iter()
            .filter(|d| matches!(d, CDecl::Function(_)))
            .collect();
        // Should recover and parse at least some functions
        assert!(funcs.len() >= 1, "should recover from parse errors");
    }

    #[test]
    fn hex_define() {
        let r = parse("#define FLAGS 0xFF00\n");
        match &r.decls[0] {
            CDecl::Define(d) => {
                assert_eq!(d.name, "FLAGS");
                assert_eq!(d.kind, CDefineKind::UnsignedInteger(0xFF00));
            }
            other => panic!("expected Define, got {:?}", other),
        }
    }

    // ---- End-to-end: realistic header ----

    #[test]
    fn realistic_header() {
        let src = r#"
#ifndef MYLIB_H
#define MYLIB_H

#include <stdint.h>

#define MYLIB_VERSION "2.0"
#define MYLIB_MAX_CONNS 128

typedef struct mylib_ctx mylib_ctx;

struct mylib_config {
    int port;
    const char *host;
    uint32_t timeout_ms;
};

typedef enum {
    MYLIB_OK = 0,
    MYLIB_ERR_CONNECT = -1,
    MYLIB_ERR_TIMEOUT = -2,
} mylib_status;

mylib_ctx *mylib_init(const struct mylib_config *cfg);
void mylib_destroy(mylib_ctx *ctx);
mylib_status mylib_send(mylib_ctx *ctx, const uint8_t *data, size_t len);

#endif
"#;
        let r = parse(src);
        let result = translate::translate(&r, &[]);

        // Should have: opaque type, struct, enum, 3 functions, 2 constants, typedef
        let funcs: Vec<_> = result.decls.iter()
            .filter(|d| matches!(d, translate::RaskCDecl::Function(_)))
            .collect();
        let structs: Vec<_> = result.decls.iter()
            .filter(|d| matches!(d, translate::RaskCDecl::Struct(_)))
            .collect();
        let enums: Vec<_> = result.decls.iter()
            .filter(|d| matches!(d, translate::RaskCDecl::Enum(_)))
            .collect();
        let consts: Vec<_> = result.decls.iter()
            .filter(|d| matches!(d, translate::RaskCDecl::Const(_)))
            .collect();

        assert!(funcs.len() >= 3, "should have at least 3 functions, got {}", funcs.len());
        assert!(structs.len() >= 1, "should have at least 1 struct, got {}", structs.len());
        assert!(enums.len() >= 1, "should have at least 1 enum, got {}", enums.len());
        assert!(consts.len() >= 2, "should have at least 2 constants, got {}", consts.len());

        // Verify specific translation
        let init_fn = funcs.iter().find(|d| {
            matches!(d, translate::RaskCDecl::Function(f) if f.name == "mylib_init")
        });
        assert!(init_fn.is_some(), "should have mylib_init");
    }

// SPDX-License-Identifier: (MIT OR Apache-2.0)

use super::*;

/// `src` as the library's own header, translated.
pub(crate) fn bind(src: &str) -> String {
    let parsed = crate::parse_c_header(src).expect("parse failed");
    translate(&parsed, &Options { header: "lib.h", hiding: &[], own: &|_| true }).source
}

fn has(out: &str, want: &str) {
    assert!(out.contains(want), "missing `{}` in:\n{}", want, out);
}

#[test]
fn functions_and_pointers() {
    let out = bind("int open(const char *path, int flags);\nvoid free(void *ptr);\nsize_t strlen(const char *s);");
    has(&out, "func open(path: *u8?, flags: c_int) -> c_int");
    has(&out, "func free(ptr: *void?)\n");
    has(&out, "func strlen(s: *u8?) -> c_size");
}

#[test]
fn variadic_and_void_params() {
    let out = bind("int printf(const char *fmt, ...);\nint getpid(void);");
    has(&out, "func printf(fmt: *u8?, ...) -> c_int");
    has(&out, "func getpid() -> c_int");
}

#[test]
fn structs_are_c_layout_and_capitalized() {
    let out = bind("struct point { int x; float y; };\nstruct sqlite3;");
    // C spells it with its tag, which a macro's C needs (PP4).
    has(&out, "@layout(C)\n@c_name(\"struct point\")\npublic struct Point {\n    public x: c_int\n    public y: f32\n}");
    has(&out, "extern \"C\" struct Sqlite3");
    // A typedef'd one is spelled as Rask spells it, so nothing is written.
    let out = bind("typedef struct Rect { int w; } Rect;\ntypedef struct { int h; } Size;");
    has(&out, "@layout(C)\npublic struct Rect {");
    has(&out, "@layout(C)\npublic struct Size {");
}

#[test]
fn unions_and_bitfields() {
    let out = bind("typedef union { int i; float f; } Bits;\ntypedef struct { unsigned flag : 1; unsigned kind : 3; } Packed;");
    has(&out, "public union Bits {\n    public i: c_int\n    public f: f32\n}");
    has(&out, "    @bits(1) public flag: c_uint\n    @bits(3) public kind: c_uint\n");
    // A union C knows only by its tag says how C spells it.
    let out = bind("union num { int i; float f; };");
    has(&out, "@c_name(\"union num\")\npublic union Num {");
}

#[test]
fn nested_anonymous_members_get_names() {
    let out = bind("struct Ev { int type; union { int key; float axis; }; struct { int x, y; } pos; };");
    has(&out, "@c_name(none)\npublic union Ev_anon0 {");
    has(&out, "public anon1: Ev_anon0");
    has(&out, "public struct Ev_anon1 {");
    has(&out, "public pos: Ev_anon1");
    has(&out, "public type_: c_int");
}

#[test]
fn enums_are_integers() {
    let out = bind("typedef enum { RED = 1, GREEN = 1 << 2, BLUE } Color;\nenum { ANON = 7 };\ntypedef enum { DOWN = -1, UP = 1 } Dir;");
    // GCC's and Clang's choice: unsigned with no negative enumerator.
    has(&out, "public type alias Color = c_uint");
    has(&out, "public type alias Dir = c_int");
    has(&out, "public const GREEN: Color = 4");
    has(&out, "public const BLUE: Color = 5");
    has(&out, "public const ANON: c_int = 7");
}

#[test]
fn macros_evaluate_or_leave_a_comment() {
    let out = bind("typedef unsigned int Uint32;\n#define A 0x01\n#define B (1 << 1)\n#define AB (A | B)\n#define W ((Uint32)0x20)\n#define BIG 0x80000000\n#define NAME \"lib\"\n#define F 1.5\n#define MAX(a, b) ((a) > (b) ? (a) : (b))\n#define ODD __attribute__((x))\n");
    has(&out, "public const A: c_int = 1");
    has(&out, "public const B: c_int = 2");
    has(&out, "public const AB: c_int = 3");
    has(&out, "public const W: c_uint = 32");
    has(&out, "public const BIG: c_uint = 2147483648");
    has(&out, "public const NAME: string = \"lib\"");
    has(&out, "public const F: f64 = 1.5");
    // A function-like macro is callable, its types from each call (PP4).
    has(&out, "@c_macro(\"((a > b) ? a : b)\")\nextern \"C\" func MAX(a: _, b: _) -> _");
}

#[test]
fn constants_through_function_like_macros() {
    let out = bind("#define U64_C(c) c ## UL\n#define FLAG_A U64_C(0x20)\n#define POS_MASK 0x2FFF0000u\n#define POS_DISPLAY(X) (POS_MASK|(X))\n#define POS_CENTERED POS_DISPLAY(0)\n#define OLD_NAME new_name\n");
    has(&out, "public const FLAG_A: c_ulong = 32");
    has(&out, "public const POS_CENTERED: c_uint = 805240832");
    assert!(!out.contains("OLD_NAME"), "a rename leaves no comment:\n{}", out);
}

#[test]
fn static_inline_gets_marked() {
    let out = bind("static inline int twice(int n) { return n * 2; }\nstatic int hidden(void) { return 1; }");
    has(&out, "@inline_from(\"lib.h\")\nextern \"C\" func twice(n: c_int) -> c_int");
    assert!(!out.contains("hidden"), "{}", out);
}

#[test]
fn callback_context_is_found() {
    let out = bind("typedef void (*Cb)(void *userdata, int n);\nvoid on_event(Cb cb, void *userdata);\nvoid qsort(void *base, size_t n, size_t sz, int (*cmp)(const void *, const void *));");
    has(&out, "@context(cb, userdata, at: 0)\nextern \"C\" func on_event(cb: Cb, userdata: *void?)");
    has(&out, "func qsort(base: *void?, n: c_size, sz: c_size, cmp: *func(*void?, *void?) -> c_int)");
}

#[test]
fn keywords_and_hiding() {
    let parsed = crate::parse_c_header("int keep(int type, int match);\nint hide(void);").unwrap();
    let out = translate(&parsed, &Options { header: "lib.h", hiding: &["hide".into()], own: &|_| true }).source;
    has(&out, "func keep(type_: c_int, match_: c_int) -> c_int");
    assert!(!out.contains("hide"), "{}", out);
}

#[test]
fn foreign_types_follow_how_they_are_used() {
    let src = "# 1 \"/usr/include/sys.h\"\nstruct timeval { long tv_sec; long tv_usec; };\nstruct big { int a; };\nint unused_sys(void);\n# 1 \"lib.h\"\nint wait(struct timeval t);\nint peek(struct big *b);\n";
    let parsed = crate::parse_c_header(src).unwrap();
    let out = translate(&parsed, &Options { header: "lib.h", hiding: &[], own: &|f| f == "lib.h" }).source;
    has(&out, "public struct Timeval {");
    has(&out, "extern \"C\" struct Big");
    assert!(!out.contains("unused_sys"), "{}", out);
}

#[test]
fn the_probe_header() {
    let out = bind(include_str!("probe.h"));
    eprintln!("{}", out);
    has(&out, "public struct Pair {\n    public a: Point\n");
    has(&out, "public const LIB_FLAG_B: c_int = 2");
    has(&out, "public type alias Cmp_fn = *func(c_int, c_int) -> c_int");
}

#[test]
fn pointer_to_pointer_parenthesizes_the_inner_optional() {
    let out = bind("void get(void **out, int **rows);");
    has(&out, "out: *(*void?)?");
    has(&out, "rows: *(*c_int?)?");
}

#[test]
fn a_float_constant_keeps_its_c_type() {
    let out = bind("#define PI_F 3.14159265358979323846f\n#define PI_D 3.14159265358979323846\n#define HALF (PI_F / 2)\n");
    has(&out, "public const PI_F: f32 = 3.1415927");
    has(&out, "public const PI_D: f64 = 3.141592653589793");
    has(&out, "public const HALF: f32 = 1.5707964");
}

#[test]
fn function_like_macros_become_callable() {
    let out = bind("#define NS_PER_MS 1000000ULL\n#define NS_TO_MS(NS) ((NS) / NS_PER_MS)\n#define LOCKED(S) (((S)->flags & 1) != 0)\n#define LOG(fmt, ...) printf(fmt, __VA_ARGS__)\nint real(int x);\n#define real(x) real(x)\n");
    // Constants go in as typed values, so the body stands on its own.
    has(&out, "@c_macro(\"(NS / ((unsigned long)1000000))\")\nextern \"C\" func NS_TO_MS(NS: _) -> _");
    // One that reads memory names its header, which the call is compiled with.
    has(&out, "@c_macro(\"(((S->flags) & 1) != 0)\", header: \"lib.h\")\nextern \"C\" func LOCKED(S: _) -> _");
    // One that isn't an expression is still declared, with the reason a call gets.
    has(&out, "@c_macro(unusable: \"takes a variable number of arguments\")");
    // A macro shadowing a function leaves the function.
    assert!(!out.contains("func real(x: _)"), "{out}");
}

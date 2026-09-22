# SPDX-License-Identifier: (MIT OR Apache-2.0)
"""Type x carrier matrix generator.

Most codegen bugs are not "feature X is broken" — they are "payload type P does
not survive carrier C". A tuple of f32 breaks while a tuple of i64 works; an
Option of f64 breaks while an Option of a struct works. One program per cell
finds those clusters in a single run, where hand-written reductions find one at
a time.

Each cell is a whole standalone program that prints the payload, round-tripped
through the carrier. Run it on both backends and diff: the interpreter is the
reference (CLAUDE.md), so a cell where native disagrees with interp is a native
bug, and a cell where both fail the same way is usually unimplemented.

Two axes:

  payloads  what is being carried — scalars, aggregates, and the things that
            aren't plain data: a Vec, a Map, a closure, a named function, a
            box, a Sequence.
  carriers  where it sits — a local, a parameter, a struct field, a Vec slot,
            an optional, a box, a yielded sequence item.

A pair the design rules out carries a SKIPS entry naming the rule, prints as
`-`, and is not run — a `Heap<T>` in a `Vec<T>` is `std.collections/C4`. That
list is deliberately tiny: a skip is a claim the compiler enforces, and a pair
that merely *fails* is not one. Those go in tests/matrix/known_red.txt with an
issue number, where the gate keeps watching them.

Usage:
    python3 tests/matrix/gen.py <outdir>          # write every cell
    python3 tests/matrix/gen.py <outdir> --types f32,f64
    python3 tests/matrix/gen.py --list            # print the runnable cell names
    python3 tests/matrix/gen.py --list-skips      # print `cell reason` per skip

tests/matrix/run.sh drives generation + both backends + the verdict.
"""

import argparse
import os
import sys

# ── Payload types ────────────────────────────────────────────────
# `decl` is the type as written in Rask, `val`/`val2` two distinct literals,
# `show` the expected stdout for `val`, `read` how to render a payload held in
# expression `{e}`. Keep the rendering exact: a cell must fail on a wrong
# value, not on float formatting.
#
# `imports` and `decls` are emitted only for the payloads that need them, so a
# cell never carries an unused import past the linter.
TYPES = {
    "i64":    dict(decl="i64",    val="42",            val2="7",      show="42"),
    "i32":    dict(decl="i32",    val="42",            val2="7",      show="42"),
    "u8":     dict(decl="u8",     val="200",           val2="7",      show="200"),
    "u64":    dict(decl="u64",    val="42",            val2="7",      show="42"),
    "f64":    dict(decl="f64",    val="2.5",           val2="1.5",    show="2.5"),
    "f32":    dict(decl="f32",    val="2.5",           val2="1.5",    show="2.5"),
    "bool":   dict(decl="bool",   val="true",          val2="false",  show="true"),
    "string": dict(decl="string", val="\"hi\"",        val2="\"yo\"", show="hi"),
    "struct": dict(decl="Pay",    val="Pay { a: 42 }", val2="Pay { a: 7 }",
                   show="Pay(42)", read="Pay({{{e}.a}})"),
    "enum":   dict(decl="Colour", val="Colour.Red",    val2="Colour.Blue",
                   show="Red", read="{{{e}.name()}}"),

    # ── Beyond scalars: the payloads a `let x = y` doesn't just memcpy. ──

    # A growable container. Non-Copy, heap-backed, released by the frame.
    # `Vec.from` rather than a bare `[7, 42]` on purpose: the literal only
    # takes its shape from an annotated `let` right now (#1233), so a bare one
    # would make eight cells measure that single inference gap instead of
    # their own carriers.
    "vec": dict(
        decl="Vec<i64>", val="Vec.from([7, 42])", val2="Vec.from([1, 2])",
        show="42", read="{{{e}[1]}}"),

    # A Map, built from pairs so it fits an expression slot like every other
    # payload (std.collections `Map.from`).
    "map": dict(
        decl="Map<string, i64>",
        val="Map.from([(\"k\", 42)])", val2="Map.from([(\"k\", 7)])",
        show="42", read="{{{e}[\"k\"]}}"),

    # A tuple — an aggregate with positional fields rather than named ones.
    "tuple": dict(
        decl="(i64, bool)", val="(42, true)", val2="(7, false)",
        show="42", read="{{{e}.0}}"),

    # A closure literal. Captures nothing, so it is legal wherever a function
    # value is; the capturing case is the `closure` carrier's job.
    "closure": dict(
        decl="func(i64) -> i64", val="|n| { return n + 40 }",
        val2="|n| { return n + 5 }", show="42", read="{{{e}(2)}}"),

    # A named function used as a value — same type as `closure`, different
    # thing underneath (no environment).
    "funcref": dict(
        decl="func(i64) -> i64", val="add_forty", val2="add_five",
        show="42", read="{{{e}(2)}}",
        decls="""\
func add_forty(n: i64) -> i64 {
    return n + 40
}

func add_five(n: i64) -> i64 {
    return n + 5
}
"""),

    # A box. `Local` is the single-task strategy, so the cell stays free of
    # the concurrency machinery (mem.boxes, conc.sync/SH2).
    "shared": dict(
        decl="Shared<i64, Local>", val="Shared.local(42)", val2="Shared.local(7)",
        show="42", read="{{{e}.get()}}",
        imports=["sync.Shared", "sync.Local"]),

    # The linear heap box. Consumed exactly once (mem.linear/L1-L3), which is
    # what rules it out of the collection carriers below.
    "heap": dict(
        decl="Heap<i64>", val="Heap(42)", val2="Heap(7)",
        show="42", read="{{*{e}}}", linear=True,
        imports=["memory.Heap"]),

    # A Sequence — a function value wearing a nominal type (type.sequence/SEQ1).
    "sequence": dict(
        decl="Sequence<i64>", val="two_items()", val2="one_item()",
        show="42", read="{{{e}.to_vec()[1]}}",
        imports=["sequence.Sequence"],
        decls="""\
func two_items() -> Sequence<i64> {
    return |yield| {
        if !yield(7) { return }
        if !yield(42) { return }
    }
}

func one_item() -> Sequence<i64> {
    return |yield| {
        if !yield(1) { return }
        if !yield(7) { return }
    }
}
"""),
}

PRELUDE = """\
// SPDX-License-Identifier: (MIT OR Apache-2.0)
// GENERATED by tests/matrix/gen.py — do not edit by hand.
"""

SHARED_DECLS = """\
struct Pay {
    a: i64
}

enum Colour {
    Red
    Blue
}

extend Colour {
    func name(self) -> string {
        match self {
            Colour.Red => { return "Red" }
            Colour.Blue => { return "Blue" }
        }
    }
}
"""


def read_expr(t, expr):
    """How to render a payload of type `t` held in `expr`, as a string interp."""
    return TYPES[t].get("read", "{{{e}}}").format(e=expr)


def commit(t, name):
    """`ensure drop(name)` for a linear payload, nothing for the rest.

    A `Heap<T>` is consumed exactly once and the ownership checker wants to see
    what will do it before the value is read (mem.linear/L1-L3, E0882). That is
    the program being correct Rask, not the carrier under test — so the carrier
    emits it and moves on."""
    return "    ensure drop(%s)\n" % name if TYPES[t].get("linear") else ""


def owning(t):
    """`own ` on a closure that gives its capture away, nothing for the rest.

    A plain closure borrows what it captures, and a borrow is not the
    closure's to hand out (mem.closures, mem.linear/L3, E0885). Nothing says
    how many times a closure runs, so a borrowing closure that returns its
    captured `Heap` would hand the same box to two callers. `own` moves the
    box in, which is the correct Rask for this cell — the carrier emits it and
    moves on, the same way `commit` emits the `ensure`."""
    return "own " if TYPES[t].get("linear") else ""


def take(t):
    """`take ` for a linear payload's parameter, nothing for the rest.

    A borrow hands the value back at the end of the call, so the caller still
    owes the consume and the callee can't return it (mem.parameters/PM1). A
    linear payload crossing a call boundary has to be taken."""
    return "take " if TYPES[t].get("linear") else ""


# ── Carriers ─────────────────────────────────────────────────────
# Each builds (extra decls, body of `main`) for one payload type.
# Every cell prints exactly one line: `got=<rendered payload>`.


def c_local(t, ty):
    return "", """\
    let x: {decl} = {val}
{commit}    println("got={show}")
""".format(decl=ty["decl"], val=ty["val"], commit=commit(t, "x"),
           show=read_expr(t, "x"))


def c_param_return(t, ty):
    decls = """\
func roundtrip({take}x: {decl}) -> {decl} {{
    return x
}}
""".format(decl=ty["decl"], take=take(t))
    return decls, """\
    let x: {decl} = {val}
    let y = roundtrip(x)
{commit}    println("got={show}")
""".format(decl=ty["decl"], val=ty["val"], commit=commit(t, "y"),
           show=read_expr(t, "y"))


def c_optional(t, ty):
    return "", """\
    let o: {decl}? = {val}
    if o? as v {{
{commit}        println("got={show}")
    }} else {{
        println("got=NONE")
    }}
""".format(decl=ty["decl"], val=ty["val"], show=read_expr(t, "v"),
           commit=("    " + commit(t, "v").lstrip("\n") if commit(t, "v") else ""))


def c_optional_param(t, ty):
    decls = """\
func unwrap_it({take}o: {decl}?) -> {decl} {{
    if o? as v {{
        return v
    }}
    return {val2}
}}
""".format(decl=ty["decl"], val2=ty["val2"], take=take(t))
    return decls, """\
    let o: {decl}? = {val}
    let v = unwrap_it(o)
{commit}    println("got={show}")
""".format(decl=ty["decl"], val=ty["val"], commit=commit(t, "v"),
           show=read_expr(t, "v"))


def c_result(t, ty):
    # An error type needs a `message` method — a primitive won't do (type.errors/ER4),
    # and Result is read with `catch`, not an `Ok`/`Err` pattern (ER2).
    decls = """\
enum Oops {{
    Bad
}}

extend Oops {{
    func message(self) -> string {{
        return "bad"
    }}
}}

func make() -> {decl} or Oops {{
    return {val}
}}
""".format(decl=ty["decl"], val=ty["val"])
    return decls, """\
    let v = make() catch _ => return
{commit}    println("got={show}")
""".format(commit=commit(t, "v"), show=read_expr(t, "v"))


def c_struct_field(t, ty):
    decls = """\
struct Holder {{
    inner: {decl}
}}
""".format(decl=ty["decl"])
    # A field holding a function value needs the call parenthesised — `h.inner(2)`
    # reads as a method call on Holder (type.structs/M6).
    src = "(h.inner)" if ty["decl"].startswith("func(") else "h.inner"
    return decls, """\
    let h = Holder {{ inner: {val} }}
{commit}    println("got={show}")
""".format(val=ty["val"], commit=commit(t, "h"), show=read_expr(t, src))


def c_vec_index(t, ty):
    return "", """\
    mut v: Vec<{decl}> = Vec.new()
    v.push({val2})
    v.push({val})
    println("got={show}")
""".format(decl=ty["decl"], val=ty["val"], val2=ty["val2"], show=read_expr(t, "v[1]"))


def c_map_value(t, ty):
    return "", """\
    mut m: Map<string, {decl}> = Map.new()
    m.insert("k", {val})
    if m.get("k")? as v {{
        println("got={show}")
    }} else {{
        println("got=MISSING")
    }}
""".format(decl=ty["decl"], val=ty["val"], show=read_expr(t, "v"))


def c_tuple(t, ty):
    return "", """\
    let tup: ({decl}, {decl}) = ({val2}, {val})
    println("got={show}")
""".format(decl=ty["decl"], val=ty["val"], val2=ty["val2"], show=read_expr(t, "tup.1"))


def c_closure_capture(t, ty):
    return "", """\
    let x: {decl} = {val}
    let f = {own}|| {{
        return x
    }}
    let y = f()
{commit}    println("got={show}")
""".format(decl=ty["decl"], val=ty["val"], own=owning(t), commit=commit(t, "y"),
           show=read_expr(t, "y"))


def c_closure_param(t, ty):
    """The payload handed *into* a closure and back out — the other direction
    from a capture, and a different lowering (mem.closures/CP1)."""
    return "", """\
    let x: {decl} = {val}
    let f = |{take}p: {decl}| {{
        return p
    }}
    let y = f(x)
{commit}    println("got={show}")
""".format(decl=ty["decl"], val=ty["val"], take=take(t), commit=commit(t, "y"),
           show=read_expr(t, "y"))


def c_own_closure(t, ty):
    """An escaping closure: `own` copies its captures into a heap environment
    (mem.closures/SL1), which is a different lowering from the borrowing one."""
    decls = """\
func escaping() -> func() -> {decl} {{
    let x: {decl} = {val}
    return own || {{
        return x
    }}
}}
""".format(decl=ty["decl"], val=ty["val"])
    return decls, """\
    let f = escaping()
    let y = f()
{commit}    println("got={show}")
""".format(commit=commit(t, "y"), show=read_expr(t, "y"))


def c_shared_box(t, ty):
    """Inside a box, read through `with` — the box family's access shape
    (mem.boxes)."""
    return "", """\
    let b: Shared<{decl}, Local> = Shared.local({val})
    with b.read() as v {{
        println("got={show}")
    }}
""".format(decl=ty["decl"], val=ty["val"], show=read_expr(t, "v"))


def c_heap_box(t, ty):
    """Inside a `Heap<T>`, read through `*` (mem.heap/HP3)."""
    return "", """\
    let b: Heap<{decl}> = Heap({val})
    ensure drop(b)
    println("got={show}")
""".format(decl=ty["decl"], val=ty["val"], show=read_expr(t, "(*b)"))


def c_seq_yield(t, ty):
    """Yielded by a Sequence and collected. `to_vec` moves what the chain owns
    and copies what is Copy (type.sequence/SEQ47)."""
    decls = """\
func pair() -> Sequence<{decl}> {{
    return |yield| {{
        if !yield({val2}) {{ return }}
        if !yield({val}) {{ return }}
    }}
}}
""".format(decl=ty["decl"], val=ty["val"], val2=ty["val2"])
    return decls, """\
    let collected = pair().to_vec()
    println("got={show}")
""".format(show=read_expr(t, "collected[1]"))


def c_for_loop(t, ty):
    """Bound by a `for` over a Vec — a loop binding, which bypasses the
    no-move-out rule that `vec[i]` answers to (ctrl.loops/LP17)."""
    return "", """\
    mut v: Vec<{decl}> = Vec.new()
    v.push({val})
    for item in v {{
        println("got={show}")
    }}
""".format(decl=ty["decl"], val=ty["val"], show=read_expr(t, "item"))


CARRIERS = {
    "local":          c_local,
    "param_return":   c_param_return,
    "optional":       c_optional,
    "optional_param": c_optional_param,
    "result":         c_result,
    "struct_field":   c_struct_field,
    "vec_index":      c_vec_index,
    "map_value":      c_map_value,
    "tuple":          c_tuple,
    "closure":        c_closure_capture,
    "closure_param":  c_closure_param,
    "own_closure":    c_own_closure,
    "shared_box":     c_shared_box,
    "heap_box":       c_heap_box,
    "seq_yield":      c_seq_yield,
    "for_loop":       c_for_loop,
}

# Carriers that need an import of their own, on top of whatever the payload
# brings.
CARRIER_IMPORTS = {
    "shared_box": ["sync.Shared", "sync.Local"],
    "heap_box":   ["memory.Heap"],
    "seq_yield":  ["sequence.Sequence"],
}


# ── Skips ────────────────────────────────────────────────────────
# Pairs that are not legal Rask, each with the rule that says so. A skip is a
# claim about the design; anything that is merely broken belongs in
# tests/matrix/known_red.txt instead, where the gate keeps watching it.
#
# The list is short on purpose, and it was much longer in the first draft. A
# `Heap<T>` in an optional, a closure or a tuple, and a `Sequence<T>` in a Vec,
# a Map or a box, all looked like they had to be out — linearity and SEQ38
# respectively. The compiler accepts every one of them, so what those skips
# were doing was hiding twenty cells behind a citation. A skip has to be a rule
# the compiler actually enforces; if the spec says no and the compiler says
# yes, that is a missing check to file, not a pair to stop generating.
def skips():
    """{(carrier, payload): reason} for every pair the design rules out."""
    # std.collections/C4: no linear resource in a Vec or a Map. The Map half is
    # enforced; the Vec half isn't yet (#1245), and both are skipped because
    # neither cell could ever go green — C4 says the program shouldn't compile.
    return {
        ("vec_index", "heap"): "std.collections/C4 — no linear resource in a Vec",
        ("map_value", "heap"): "std.collections/C4 — no linear resource in a Map",
        # Both of these reach the payload through a `Vec`, so C4 rules them out
        # for the same reason: `for x in v` needs the vector, and the sequence
        # carrier reads its items back with `to_vec()`.
        ("for_loop", "heap"): "std.collections/C4 — no linear resource in a Vec",
        ("seq_yield", "heap"): "std.collections/C4 — the items come back in a Vec",
        # A borrow hands the value back when the call returns, so the closure
        # can't return it, and CP4 says a closure can't take it either — there
        # is no spelling of "a Heap handed into a closure and back out".
        ("closure_param", "heap"):
            "mem.closures/CP4 — a closure can't take ownership through a parameter",
    }


SKIPS = skips()


def expected(t):
    """The single line a correct cell prints."""
    return "got=" + TYPES[t]["show"]


def cell(t, c):
    ty = TYPES[t]
    decls, body = CARRIERS[c](t, ty)

    imports = []
    for name in list(ty.get("imports", ())) + CARRIER_IMPORTS.get(c, []):
        if name not in imports:
            imports.append(name)
    head = "".join("import %s\n" % name for name in imports)

    # The shared Pay/Colour declarations are only worth emitting for the
    # payloads that name them — an unused struct is lint noise in 200 files.
    shared = SHARED_DECLS if t in ("struct", "enum") else ""
    own = ty.get("decls", "")

    parts = [p.rstrip("\n") for p in (head, shared, own, decls) if p.strip()]
    parts.append("func main() {\n%s}" % body)
    return PRELUDE + "\n\n".join(parts) + "\n"


def axes(args):
    types = args.types.split(",") if args.types else list(TYPES)
    carriers = args.carriers.split(",") if args.carriers else list(CARRIERS)
    for t in types:
        if t not in TYPES:
            sys.exit("unknown payload type: %s (have %s)" % (t, ", ".join(TYPES)))
    for c in carriers:
        if c not in CARRIERS:
            sys.exit("unknown carrier: %s (have %s)" % (c, ", ".join(CARRIERS)))
    return types, carriers


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("outdir", nargs="?")
    ap.add_argument("--types", help="comma-separated payload types (default all)")
    ap.add_argument("--carriers", help="comma-separated carriers (default all)")
    ap.add_argument("--list", action="store_true", help="print runnable cell names")
    ap.add_argument("--list-skips", action="store_true",
                    help="print `cell reason` for each pair the design rules out")
    args = ap.parse_args()

    types, carriers = axes(args)

    if args.list_skips:
        for t in types:
            for c in carriers:
                if (c, t) in SKIPS:
                    print("%s__%s %s" % (c, t, SKIPS[(c, t)]))
        return

    if args.list:
        for t in types:
            for c in carriers:
                if (c, t) not in SKIPS:
                    print("%s__%s" % (c, t))
        return

    if not args.outdir:
        sys.exit("outdir required (or pass --list)")
    os.makedirs(args.outdir, exist_ok=True)
    n = 0
    for t in types:
        for c in carriers:
            if (c, t) in SKIPS:
                continue
            name = "%s__%s" % (c, t)
            with open(os.path.join(args.outdir, name + ".rk"), "w") as f:
                f.write(cell(t, c))
            with open(os.path.join(args.outdir, name + ".expected"), "w") as f:
                f.write(expected(t) + "\n")
            n += 1
    print("wrote %d cells to %s" % (n, args.outdir))


if __name__ == "__main__":
    main()

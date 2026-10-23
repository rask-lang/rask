<!-- id: type.reads-as -->
<!-- status: decided -->
<!-- summary: A struct may read as one of its own fields: methods it doesn't declare are that field's read-only methods, called on the field -->
<!-- depends: types/structs.md, memory/parameters.md -->
<!-- implemented-by: compiler/crates/rask-types/src/checker/reads_as.rs -->

# Reads-As

A wrapper that only adds a little to a value it holds has to forward every read
of that value by hand, or give up and expose the field. `@reads_as(field)` says
it once, on the type: a method the struct doesn't declare is the field's, called
on the field.

<!-- test: parse -->
```rask
@reads_as(text)
struct Label {
    text: string
    color: u32
}

func width(l: Label) -> usize {
    return l.len()              // l.text.len()
}
```

| Rule | Description |
|------|-------------|
| **RA1: A field, named on the type** | `@reads_as(name)` on a struct names one of its own fields. The struct reads as that field, and nothing else does it for the struct: there is no conversion method to write and none is called. A name the struct has no field for is an error (E0305) |
| **RA2: The type's own methods first** | A method the struct declares is the struct's. Only a name it doesn't declare is looked up on the field's type |
| **RA3: One step** | The field's type reading as something else isn't followed. `a.m()` is `a.field.m()` or nothing |
| **RA4: Only what reads** | A method lent this way takes `self` as a plain borrow. One that takes `mutate self` or `take self` isn't lent (E0307): the struct would change through a name that only promised to read it |
| **RA5: Receivers only** | Reading through the field happens at a method call's receiver, and at the operands of a comparison (RA6), and nowhere else. A value passed where the field's type is expected is a type error, and the error names the struct's own conversions to that type. The callee can keep a parameter, so handing it the field would let it keep part of the struct without saying so |
| **RA6: Comparing is reading** | A struct that reads as a field gets no structural `eq`, `hash` or `compare`. Both operands of `==`, `!=`, `<`, `<=`, `>` and `>=` read through their field, so `a == "x"`, `"x" == a` and `a == b` all compare the fields. That is the one place an operand that isn't the receiver is read through: a comparison reads both sides and answers a `bool`, so it can't keep either, and RA5's reason doesn't apply. It isn't `Equal` or `Hashable` as itself, so it can't key a map |

## Why it isn't `Deref`

Rust's `Deref` is a method the type implements, so it can run code; it chains
(`Box<Rc<String>>` reaches `str`); it lends `&mut` access through `DerefMut`;
and it applies at argument sites by coercion. Each of those hides something.
Reads-as is declared on the type, names a field so the conversion is a field read
and free by construction, stops after one step, lends only reads, and works on
receivers and comparison operands only. What it lends is visible at the type's declaration, and what it
costs is nothing.

## In the stdlib

`StringView` and `StringSlice` read as the `string` they hold (`std.strings/V3`,
SL2). That is the whole of "a slice reads as a string": every read-only string
method on a slice is the string method, called on the slice's text.

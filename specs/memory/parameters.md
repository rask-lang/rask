<!-- id: mem.parameters -->
<!-- status: decided -->
<!-- summary: Three parameter modes — borrow (default), mutate, take. Mutate arguments are marked at the call site too -->
<!-- depends: memory/ownership.md, memory/borrowing.md -->
<!-- implemented-by: compiler/crates/rask-types/, compiler/crates/rask-parser/ -->

# Parameter Modes

Four modes: **borrow** (default, read-only), **mutate** (explicit mutable borrow), **deleting** (mutate, and may delete from a `Rack` the caller has links into), **take** (ownership transfer). Anything that changes what the caller may assume is marked at both ends: the word in the signature *and* on the argument at the call site.

## Modes

| Rule | Mode | Signature | Call site | Caller After |
|------|------|-----------|-----------|--------------|
| **PM1: Borrow** | Borrow | `param: T` | `f(x)` | Value still valid |
| **PM2: Mutate** | Mutate | `mutate param: T` | `f(mutate x)` — marker required (PM4) | Value still valid |
| **PM2b: Deleting** | Mutate + delete | `deleting param: T` | `f(deleting x)` — marker required (PM4) | Value still valid; links into it are not |
| **PM3: Take** | Take | `take param: T` | `f(x)` or `f(own x)` — marker optional | Value invalid |

| Rule | Description |
|------|-------------|
| **PM4: Call-site mutate marker** | An argument passed to a `mutate` parameter is written `mutate arg` at the call site. Omitting it is a compile error with the one-token fix. Method receivers are exempt: `player.take_damage(10)` needs no marker — the receiver is understood to be the thing operated on |
| **PM5: Marker follows the signature** | PM4 is syntactic: the marker is required exactly when the parameter is declared `mutate` or `deleting`, and it is *that* word at the call site, regardless of the argument's type. A Copy argument to a `mutate` parameter still writes `mutate` — the rule never depends on a type's size |
| **PM6: A borrow can't be given away** | A `param: T` cannot be consumed inside the body — not by a `take` parameter, not by a `take self` method, not by storing it into a field or another aggregate, and `own` at the inner call site changes nothing. The caller keeps the value and goes on using it, so consuming it would leave them holding something that's gone (`mem.linear/L1`). Compile error at the consumption, pointing at the declaration; `take` on the declaration is the fix |
| **PM7: A consumed `mutate` parameter is replaced** | A `mutate` parameter *may* be consumed — exclusive access is what makes taking the value out and writing a replacement back the mode's whole point. PM2 promises the value is still there when the call returns, so a replacement has to be assigned on every path that reaches the return. Consumed on some paths and replaced on none, or on only some, is a compile error. A function that keeps the value for good declares `take` instead, so the call site shows it going |
| **PM8: `deleting` implies `mutate`** | Deleting a node mutates the rack it lives in, so `deleting` grants everything `mutate` does. Writing both parses and is redundant; `deleting param: T` alone is the idiom. The two are a lattice — `param` → `mutate param` → `deleting param` — not independent axes |
| **PM9: What `deleting` is for** | A callee may delete a link the caller handed over as a `take` parameter with no annotation: the name is consumed at the call site, so the caller watches it die. `deleting` covers the case the caller cannot see — the callee picking its own victims, by iterating the rack, by `clear`, or by handing a link it derived to something that consumes it. At such a call every link the caller holds into that rack is revoked, because which nodes died is not knowable from outside |

### A `Link<T>` parameter is a view or a writer

`mutate` on a link means "I will write the node", not "I will change the link".
That distinction does real work, because it is what a read-only link is:

<!-- test: skip -->
```rask
// A view. Reads the node and everything reachable from it, writes none of it.
func total(n: Link<Node>) -> i32 {
    mut t = n.value
    if n.child? as c { t = t + c.value }
    return t
}

// A writer. Says so in the signature; needs no rack in scope.
func zero_all(mutate n: Link<Node>) {
    n.value = 0
    if n.child? as c { c.value = 0 }
}

let a = rack.insert(Node { value: 5, child: none })
zero_all(mutate a)              // `let a` — see PM10
```

Two properties make the view worth having, and both are checked. A view cannot
be passed on as `mutate`, so it can't be laundered into a writer in one hop. And
it stays a view when you follow an edge — `c` above inherits nothing, because no
permission is the default.

Write permission travels the other way: it propagates *outward* along edges, so
a writer may write anything reachable. An edge only connects nodes in one rack,
so if you may write this node you may write the ones it points at.

This is why there is no `LinkView<T>`. A read-only *type* would have to either
propagate along every edge or leak in one hop, and it would put `mut` in a type
position, which no other box does. The mode is the same borrow-versus-mutate
distinction every other type already has.

| Rule | Description |
|------|-------------|
| **PM10: A link's mutability is the node's** | `mutate n: Link<T>` grants writing the node the link points at; the link itself is a pointer and is not modified. So a `let` binding may be passed as `mutate` when the parameter is a link — demanding `mut` there would ask permission to change the one thing that isn't changing. A read-only *parameter* is not exempt: passing `n: Link<T>` on as `mutate` is an error, or a view would launder into a writer |

### Deleting Mode

`deleting` exists because `Rack<T>` hands out `Link<T>`, and a link is a pointer to a node rather than a ticket to look one up. A delete the caller can see is safe without ceremony; a delete it cannot see is a use after free. The mode is what makes the second case visible.

<!-- test: skip -->
```rask
// Deletes exactly what it was handed — no annotation. The `take` already tells
// the caller which node died.
func remove(mutate list: List, take n: Link<Node>) {
    if n.prev? as p { p.next = n.next } else { list.head = n.next }
    list.nodes.delete(n)
}

// Picks its own nodes, so it says so — and the call revokes the caller's links.
func delete_subtree(deleting scene: Scene, take n: Link<SceneNode>) {
    let kids = n.children.clone()
    for c in kids { delete_subtree(deleting scene, c) }
    scene.nodes.delete(n)
}
```

The word is contextual, not reserved: it is a mode only when a parameter name or another mode word follows it, so `func d(deleting: i32)` and a field named `deleting` keep working.

Omitting it where it is needed is a compile error in the callee (E0329), naming the parameter to declare. Writing `mutate` where the signature says `deleting` is an error at the call site (E0330), with the one-token fix — two different contracts should not print the same.

**Status:** the mode is accepted and implemented; `Rack<T>`/`Link<T>` themselves are still an exploration (`analysis.fourth-option`) with no normative spec and no native lowering.

### Borrow Mode (Default)

Function gets read-only access; caller keeps ownership. Compiler enforces immutability.

<!-- test: skip -->
```rask
func process(data: Data) -> Report {
    // Can read data.field, call data.method()
    // Cannot mutate data or give it away
    Report.from(data)
}

let d = Data.new()
process(d)      // d borrowed (read-only)
print(d.name)   // OK: d still valid
```

### Mutate Mode

Explicit mutable borrow. Function can modify the value; caller keeps ownership. This includes field mutation, method calls, **and full reassignment** of the parameter.

<!-- test: skip -->
```rask
func apply_damage(mutate player: Player, amount: i32) {
    player.health -= amount              // field mutation
    player.last_hit = now()              // field mutation
    if player.health <= 0 {
        player.status = Status.Dead      // field mutation
    }
}

func reset(mutate player: Player) {
    player = Player.new()                // full reassignment — allowed
}

mut player = Player.new()
apply_damage(mutate player, 10)          // marker required at the call site (PM4)
reset(mutate player)
player.take_damage(10)                   // receiver: no marker (PM4)
```

### Take Mode

Ownership transfer. Caller gives up the value.

<!-- test: skip -->
```rask
func consume(take data: Data) {
    // Can do anything: store, send, drop
    storage.rack(data)
}

let d = Data.new()
consume(d)      // d taken
print(d.name)   // ERROR: d was taken
```

## Self Parameter

| Syntax | Meaning |
|--------|---------|
| `self` | Read-only self (enforced) |
| `mutate self` | Mutable self |
| `take self` | Take ownership (consuming method) |

<!-- test: skip -->
```rask
extend File {
    func size(self) -> usize {
        self.metadata.size
    }

    func read(mutate self, buf: [u8]) -> usize or Error {
        // reads from self (mutates internal position)
    }

    func close(take self) -> void or Error {
        // closes and invalidates self
    }
}

let file = try File.open("data.txt")
try file.read(buf)    // mutably borrows file
try file.read(buf)    // OK: can borrow again
try file.close()      // takes file
try file.read(buf)    // ERROR: file was taken
```

## Disjoint Field Borrows

When passing `value.field` to a `mutate` parameter, the borrow checker tracks the borrow at field granularity. Functions take the field's concrete type — no special projection syntax needed.

<!-- test: skip -->
```rask
func heal(mutate health: Health) {
    health.current += 10
}

func loot(mutate inventory: Inventory) {
    inventory.push(item)
}

func update(mutate player: Player) {
    heal(mutate player.health)         // Borrows player.health
    loot(mutate player.inventory)      // OK: borrows player.inventory (disjoint)
}
```

See `mem.borrowing/F1`–`F4` for the full disjoint field borrowing rules.

## Interaction with Copy Types

For Copy types (≤16 bytes), values are copied in. The mode distinction matters for non-Copy types.

<!-- test: parse -->
```rask
func process(x: i32) {
    // x is copied in, caller keeps original
}

func process(take x: i32) {
    // Also copied, but semantically "taken"
    // Useful for move-only small types
}
```

## Interaction with Resource Types

Resource types (`mem.resources/R1`) must be consumed exactly once. Only `take` parameters can consume them.

<!-- test: skip -->
```rask
@resource
struct File { ... }

func process(file: File) {         // Read-only borrow
    try file.read()              // OK: reading
}   // file returned to caller

func finish(take file: File) {     // Take
    try file.close()             // OK: consuming
}   // file consumed

let f = try File.open(path)
process(f)     // borrows f (read-only)
finish(f)      // takes f, f now invalid
```

## Error Messages

**Mutating a read-only parameter [PM1]:**
```
ERROR [mem.parameters/PM1]: cannot mutate parameter 'data'
   |
5  |  func update(data: Data) {
   |              ^^^^ 'data' is read-only (default)
6  |      data.count += 1
   |      ^^^^^^^^^^^^^^^ cannot assign to field of read-only parameter

FIX: Add 'mutate' to allow mutation:
   |
5  |  func update(mutate data: Data) {
```

**Missing call-site marker [PM4]:**
```
ERROR [mem.parameters/PM4]: `apply_damage` mutates `player` — mark it at the call site
   |
9  |  apply_damage(player, 10)
   |               ^^^^^^ passed to a `mutate` parameter
   |
4  |  func apply_damage(mutate player: Player, amount: i32)
   |                    ------ declared here

FIX: apply_damage(mutate player, 10)
```

**Taking a borrowed parameter [PM3]:**
```
ERROR [mem.parameters/PM3]: cannot take ownership of borrowed parameter
   |
5  |  func process(data: Data) {
   |             ^^^^ 'data' is borrowed, not taken
6  |      storage.rack(data)
   |                    ^^^^ 'rack' takes ownership

FIX: Add 'take' to receive ownership:
   |
5  |  func process(take data: Data) {
```

**Using after taken [PM3]:**
```
ERROR [mem.parameters/PM3]: value used after being taken
   |
5  |  consume(data)
   |          ^^^^ 'data' taken here
6  |  print(data.name)
   |        ^^^^ cannot use 'data' after it was taken
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Generic parameters | PM1–PM3 | Mode applies to concrete type at instantiation |
| Closure captures | — | Captured borrows follow closure lifetime rules (`mem.closures`) |
| Pattern matching | PM2 | Mutation only allowed if parameter is `mutate` |
| Copy type + mutate | PM2/PM5 | Value is copied in; mutations affect the copy. Call site still writes `mutate` — the marker follows the signature, not the size |
| Disjoint field borrows | — | Passing `mutate value.field` borrows only that field (`mem.borrowing/F1`) |
| Method receiver | PM4 | Exempt — `x.method()` never marks the receiver, even for `mutate self` |
| `mutate` marker on a borrow argument | PM4 | Compile error — marker without a `mutate` parameter is a lie the compiler rejects |
| `mutate` marker on a `let` binding | PM2 | Compile error — `let` is deep; bind with `mut` first |

---

## Appendix (non-normative)

### Rationale

**PM1 (borrow default):** Borrowing is ~85% of parameters, and most borrows are read-only. I made the default read-only because mutation should be visible — if a function changes your data, you should see that in the signature.

**PM2 (mutate):** `mutate` marks intent: "I will change this parameter." This includes field-level modification AND full reassignment — there's no half-mutable state. A `mutate` parameter gives you unrestricted write access to the value; the constraint is that the caller keeps ownership after the call.

Note the interaction with `let` bindings: `let` is deep — you cannot pass a `let` binding as a `mutate` argument, call a `mutate self` method on it, or assign through an index/field. Rebinding and all forms of mutation through the binding name are rejected. If you need to pass a value to a `mutate` parameter, bind it with `mut`. Moving a const-bound value to a `take` parameter is allowed — ownership transfer is not mutation.

**PM3 (take):** The rare case. Ownership transfer only when you need to store, send, or consume.

**PM4 (call-site `mutate` markers — this flipped).** Swift (`&x`), C# (`ref x`), and Rue (`&x`) all require markers at call sites for mutable parameters. The first version of this spec chose against, on three arguments: ceremony is per-call not per-definition; `own` marks the destructive case so mutation (the reversible one) can stay quiet; and tooling shows the mode at call sites anyway. That reasoning is preserved in history because knowing why it lost matters.

It lost to one observation: **mark what the compiler can't backstop.** Misread a move and your next use of the value is a compile error — the checker corrects the wrong belief, which is why `own` can stay optional. Misread a mutation and nothing corrects you: `apply_damage(player, 10)` compiles identically whether `player` changes or not, and a reviewer's wrong belief survives all the way to production. The old rationale marked the irreversible action; the irreversible action was the one that never needed marking. Ceremony belongs exactly where a wrong reading is *legal*.

The cost stayed small for the same reason the old rationale said it would: most mutation flows through receivers (`vec.push(x)`, `player.take_damage(10)`), which are exempt — the receiver is the thing being operated on, the universal convention in Go, Swift, and Rust alike. What PM4 marks is the rarer, easily-missed case: a free function (or a non-receiver argument) that reaches in and changes something you passed. One word, at the exact sites a plain-diff reviewer would otherwise have to look up.

`take` arguments keep the optional `own` marker: write it for emphasis, or let the checker's use-after-move errors do the guarding. Making it required would mark the backstopped case — the mistake the old PM4 rationale made, inverted.

The three conditions this decision fell out of — wrong reading is legal, mark is non-viral, marked case is the minority — are now the general rule for all explicitness debates: see "The Ceremony Test" in [CORE_DESIGN.md](../CORE_DESIGN.md).

### Patterns & Guidance

**Method chains:**

<!-- test: skip -->
```rask
extend Builder {
    func name(mutate self, n: string) -> Self {
        self.name = n
        self
    }

    func build(take self) -> Widget {
        Widget.new(self.name)
    }
}

Builder.new()
    .name("foo")      // mutably borrows, returns self
    .name("bar")      // mutably borrows, returns self
    .build()          // takes, returns Widget
```

### IDE Integration

**Signatures:** All modes are visible in source — no ghost annotations needed.

**Call sites:** `mutate` is in source (PM4), so the only ghost left is `own` on unmarked take arguments:

<!-- test: skip -->
```rask
apply_damage(mutate player, 10) // in source — nothing to ghost
consume(user)                   // IDE shows: consume(own user)  [nothing if own written]
process(data)                   // IDE shows nothing (borrow is default, no annotation)
```

| Context | Ghost annotation |
|---------|-----------------|
| Borrow argument | None (default, no noise) |
| `mutate` argument | None — PM4 puts it in source |
| `take` argument | `own` ghost before argument (nothing if `own` already written) |

Mutation is visible on every surface — source, diff, grep — with no tooling required. The `own` ghost (and `rask annotate`'s `own` row) covers the one remaining call-site mode, where the checker already guards against misreading.

### See Also

- [Value Semantics](value-semantics.md) — Copy vs move behavior (`mem.value`)
- [Linearity](linear.md) — Only `take` parameters can consume linear values (`mem.linear`)
- [Resource Types](resource-types.md) — `@resource` annotation (`mem.resources`)
- [Borrowing](borrowing.md) — Borrow scope rules (`mem.borrowing`)
- [Closures](closures.md) — Closure parameter modes (`mem.closures`)
- [Boxes](boxes.md) — Box parameters move ownership like any other value (`mem.boxes`)
- [Structs](../types/structs.md) — Struct definition, methods (`type.structs`)

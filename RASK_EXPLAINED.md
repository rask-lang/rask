# Rask Explained

A simple guide to how Rask works.

## The Big Idea

In most languages, you can create pointers—variables that "point to" other variables. This causes bugs:
- What if the thing you're pointing to gets deleted? (crash)
- What if two threads modify it at the same time? (corruption)
- What if you forget to delete it? (memory leak)

Rask's solution: **you can't store pointers.** Ever.

**Design principle:** If the compiler knows it, you don't write it. Function signatures declare intent (borrow, take, inout). Callsites are clean—no redundant markers. Your IDE shows ghost labels for visibility when you want it.

Instead:
- You **own** values (they're yours, you can do whatever)
- You **borrow** values temporarily (look but return it)
- You use **keys** to find things in collections (like a coat check ticket)

That's it. No garbage collector, no complex lifetime rules, no runtime checks.

## Ownership: It's Yours

When you create a value, you own it:

```
user = User{name: "Alice", age: 30}
```

You can use it, change it, whatever. It's yours.

## Borrowing: Let Me See That (Default)

When you pass a value to a function, by default it just **borrows** it:

```
fn print_user(u: User) {   // No keyword = just looking
    print(u.name)
}

user = User{name: "Alice", age: 30}
print_user(user)     // borrows user temporarily
print(user.name)     // still works! you still own user
```

Most functions just need to read data, so borrowing is the default.

## Taking: Give It Away

If a function needs to own the value, it uses `take`:

```
fn delete_user(u: take User) { ... }   // "take" = I'm keeping this

user = User{name: "Alice", age: 30}
delete_user(user)    // user is now GONE
print(user.name)     // ERROR! you don't own user anymore
```

## Modifying: In-Out

If a function needs to modify a value and give it back, use `inout`:

```
fn birthday(u: inout User) {
    u.age = u.age + 1
}

user = User{name: "Alice", age: 30}
birthday(user)       // Modifies user in place
print(user.age)      // prints 31
```

The function signature says `inout`, so the compiler knows. No special syntax at the callsite—your IDE can show ghost labels like `birthday(inout: user)` if you want visibility.

## Small Values Copy Automatically

For small things like numbers, copying is automatic:

```
x = 5
y = x        // copies the value
print(x)     // still works, x is still 5
```

This only happens for small types (numbers, small structs). Big things must be explicitly cloned.

## Collections Use Keys

In other languages, you might store a reference to an item in a list:

```
// Other languages (dangerous!)
item = list[5]       // item "points to" the 5th element
list.clear()         // delete everything
print(item)          // CRASH - item points to deleted memory
```

In Rask, you get a **key** instead:

```
// Rask (safe)
key = 5              // just a number
item = list[key]     // looks up the item NOW
list.clear()         // delete everything
item = list[key]     // returns None - key no longer valid
```

Keys are just numbers. They can't "dangle" because they're not pointers.

## Linear Types: Must Use Once

Some things (files, network connections) MUST be properly closed:

```
file = open("data.txt")
// ... use file ...
close(file)         // MUST do this - takes ownership
```

Rask enforces this. If you forget to close a file, it's a compile error:

```
fn bad() {
    file = open("data.txt")
    // ERROR: file was never closed!
}
```

You can still read info from linear types:

```
fn print_file_size(f: FileHandle) {  // borrow - just looking
    print(f.size())
}

file = open("data.txt")
print_file_size(file)   // OK - just borrowing
close(file)             // still must close it (takes ownership)
```

## Tasks Don't Share

Each task (like a thread) owns its own data. To send data to another task:

```
channel.send(my_data)   // my_data is GONE from this task
// ... other task receives it ...
```

Because data moves (not copies), two tasks can never access the same data. No locks needed, no race conditions possible.

## Arenas: Bulk Memory

When you need lots of small allocations (like parsing a file), use an arena:

```
arena = Arena.new()

// allocate many things
for line in file {
    arena.alloc(parse(line))
}

// use them...

arena.free()   // everything freed at once
```

Fast allocation, fast cleanup, no individual frees needed.

## Traits: Shared Behavior

Traits let you write code that works with multiple types:

```
trait Drawable {
    fn draw(observe self)
}

impl Drawable for Circle {
    fn draw(observe self) { /* draw circle */ }
}

impl Drawable for Square {
    fn draw(observe self) { /* draw square */ }
}

fn draw_anything<T: Drawable>(shape: observe T) {
    shape.draw()   // works for Circle, Square, anything Drawable
}
```

You can add traits to types you didn't write:

```
impl Drawable for SomeLibraryType {
    fn draw(observe self) { /* your implementation */ }
}
```

## Summary

| Concept | What it means |
|---------|---------------|
| **Own** | It's yours, do whatever |
| **Borrow** (default) | Look at it, caller keeps it |
| **Take** (`take T`) | Take ownership, caller loses it |
| **Inout** (`inout T`) | Modify in place, caller keeps modified value |
| **Copy** | Small values copy automatically |
| **Key** | Number that finds things in collections |
| **Linear** | Must be consumed exactly once (files, etc.) |
| **Arena** | Allocate many, free all at once |
| **Trait** | Shared behavior across types |

## The Tradeoffs

What Rask makes **harder**:
- Graph structures (nodes pointing to each other) - use keys instead
- Sharing data between tasks - use channels to transfer

What Rask makes **easier**:
- No null pointer crashes
- No use-after-free bugs
- No data races
- No memory leaks (for linear types)
- No fighting with a borrow checker
- No lifetime annotations
- Clear data flow (values in, values out)

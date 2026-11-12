# Your first program

`hello.rk`, in this directory, is a whole program:

<!-- from: hello.rk -->
```rask
func main() {
    let name = "Rask"
    println("Hello, {name}!")
}
```

Run it:

```bash
rask run hello.rk
```

<!-- from: output/03-hello.out -->
```text
Hello, Rask!
```

`rask run` compiles the file to a native program and runs it. A program starts in the
function called `main`.

`let name = "Rask"` gives a value a name. `println` prints a line, and a string can hold
values in braces: `"{name}"` is replaced by whatever `name` holds. That's the only way to
build a string out of pieces. There's no `+` for strings, so a string that came from
several places always shows all of them in one place.

## Names that change

A name made with `let` holds its value for good. Nothing about that value changes through
it, so when you read `let`, you know what the name holds for the rest of the block. A name
that needs to change is made with `mut`:

<!-- test: run | 3 rooms visited -->
```rask
func main() {
    mut visited = 0
    visited += 1
    visited += 2
    println("{visited} rooms visited")
}
```

The type is worked out from the value. You can write it when you want it on the page:
`mut visited: i64 = 0`.

## Functions

A function says what it takes and what it gives back, and it gives it back with `return`:

<!-- test: run | Gate: A rusted iron gate. -->
```rask
func title(name: string, description: string) -> string {
    return "{name}: {description}"
}

func main() {
    println(title("Gate", "A rusted iron gate."))
}
```

Every function that gives back a value says `return`. A block's last line doesn't
quietly become its result, so the place a function hands its answer back is always
written down.

## Exercise

`03-your-first-program.rk` is a program with an empty `main`. Run it as it is:

```bash
rask run 03-your-first-program.rk
```

<!-- from: output/03-exercise.out -->
```text
panic at 03-your-first-program.rk:12: not yet implemented
```

That's `todo()`, the body every exercise starts with. It stops the program and says where.
Replace it with a `main` that prints the first room of the game, its name on one line and
its description on the next. The finished version is `early/first.rk`, and it prints this:

```bash
rask run early/first.rk
```

<!-- from: output/03-solved.out -->
```text
Gate
A rusted iron gate stands open to the north.
```

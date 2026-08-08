# Text Editor

A text editor with undo/redo functionality.

**Full source:** [text_editor.rk](https://github.com/rask-lang/rask/blob/main/examples/text_editor.rk)

## Key Concepts Demonstrated

- Command pattern for undo/redo
- File I/O and resource management
- State transitions
- Vec usage for history

## Highlights

### Command Pattern

<!-- test: parse -->
```rask
enum EditCommand {
    Insert(i32, string),
    Delete(i32, string),
    Modify(i32, string, string),
}

struct Document {
    lines: Vec<string>
    history: Vec<EditCommand>
    position: i32
}
```

### Undo/Redo

Undo and redo are methods on `Document`, so the receiver carries the mutation — no call-site
marker needed:

<!-- test: parse -->
```rask
extend Document {
    func undo(self) -> bool or Error {
        if self.position <= 0: return false

        self.position -= 1
        let cmd = self.history[self.position]
        try self.apply_command_silent(cmd.inverse())
        return true
    }

    func redo(self) -> bool or Error {
        if self.position >= self.history.len(): return false

        let cmd = self.history[self.position]
        try self.apply_command_silent(cmd)
        self.position += 1
        return true
    }
}
```

Both return `bool or Error` — `true` when something was undone, `false` at the end of the
history. A bare `return true` auto-wraps into the success branch; there's no `Ok(...)` to write.

If these were free functions instead of methods, the document parameter would need `mutate` in
the signature *and* at every call site: `func undo(mutate doc: Document)` called as
`undo(mutate doc)`. Methods are the exception — the receiver is understood to be the thing
being operated on.

### File Operations

<!-- test: parse -->
```rask
func save(self, path: string) -> void or fs.IoError {
    let file = try fs.create(path)
    ensure file.close()

    try file.write(self.lines.join("\n"))
}

func from_file(path: string) -> Document or fs.IoError {
    let file = try fs.open(path)
    ensure file.close()

    let content = try file.read_to_string()
    return Document { lines: content.split("\n"), history: Vec.new(), position: 0 }
}
```

The empty return type is spelled `void`, and `ensure file.close()` satisfies the linearity rule
on every exit path — including the ones `try` takes.

## Running It

```bash
rask run examples/text_editor.rk
```

## What You'll Learn

- Command pattern for undo/redo
- Resource management with files
- State management in Rask
- Vec operations for history tracking

[View full source →](https://github.com/rask-lang/rask/blob/main/examples/text_editor.rk)

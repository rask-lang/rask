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

```rask
enum Command {
    Insert(usize, string),
    Delete(usize, usize),
    Replace(usize, usize, string),
}

struct Editor {
    content: string,
    history: Vec<Command>,
    position: usize,
}
```

### Undo/Redo

```rask
func undo(editor: Editor) {
    if editor.position > 0 {
        editor.position -= 1
        const cmd = editor.history[editor.position]
        reverse_command(editor, cmd)
    }
}

func redo(editor: Editor) {
    if editor.position < editor.history.len() {
        const cmd = editor.history[editor.position]
        apply_command(editor, cmd)
        editor.position += 1
    }
}
```

### File Operations

```rask
func save(editor: Editor, path: string) -> () or IoError {
    const file = try fs.create(path)
    ensure file.close()

    try file.write(editor.content)
}

func load(path: string) -> Editor or IoError {
    const file = try fs.open(path)
    ensure file.close()

    const content = try file.read_to_string()
    return Editor { content, history: Vec.new(), position: 0 }
}
```

## Running It

```bash
rask text_editor.rk
```

## What You'll Learn

- Command pattern for undo/redo
- Resource management with files
- State management in Rask
- Vec operations for history tracking

[View full source →](https://github.com/rask-lang/rask/blob/main/examples/text_editor.rk)

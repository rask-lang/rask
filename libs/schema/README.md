# schema

TOML config files checked against a versioned schema. The schema is the current shape plus an append-only history of changes, so a file written against any past version still loads, and a file can be migrated to another version in either direction.

The toolchain uses it for `rask.toml` (`compiler/crates/rask-manifest/loader/` holds that schema), but nothing here knows about packages.

```rask
import schema.{Schema, Ty, Field, Release, Change, Value, required, defaulted, root, load, check_history, str_at}

func config_fields() -> Vec<Field> {
    return Vec<Field>.from([
        required("name", Ty.Str)
        defaulted("timeout", Ty.Duration, Value.Str("30s"))
    ])
}

// Release 1 adds every field; later releases append their changes.
func config_schema() -> Schema {
    mut adds = Vec<Change>.new()
    mut fields = config_fields()
    for f in fields.take_all() {
        adds.push(Change.Add(at: root(), field: f))
    }
    return Schema(
        name: "myapp-config"
        shape: Ty.Record(config_fields())
        history: Vec<Release>.from([Release(version: 1, changes: adds)])
    )
}

func main() {
    check_history(config_schema()) catch e => panic(e.message())
    let tree = load("schema = \"myapp-config@1\"\nname = \"api\"\n", config_schema()) catch e => panic(e.message())
    println("{str_at(tree, "name") ?? ""}")   // api
}
```

A file starts with `schema = "myapp-config@1"`. Loading reads that version, replays the history forward, and checks the result against the current shape. Every problem comes back at once, each with its dotted path, and unknown keys name the nearest valid one. `check_history` replays the whole log from nothing and fails if it doesn't land on the written shape, so put it in a test.

| File | Holds |
|------|-------|
| `toml.rk` | The TOML subset reader and writer, well-formedness checks, typed reads of a tree |
| `schema.rk` | Shapes, changes, replaying the log on shapes, the history check |
| `check.rk` | Checking a tree against a shape, with suggestions |
| `replay.rk` | Load and migrate: the log run over a file, either direction |
| `jsonschema.rk` | The current shape as JSON Schema, for editors |

The rules it follows are written up in [struct.manifest](../../specs/structure/manifest.md).

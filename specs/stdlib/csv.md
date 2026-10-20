<!-- id: std.csv -->
<!-- status: decided -->
<!-- summary: CSV reading and writing — typed decode into structs first, raw rows as the fallback -->
<!-- depends: stdlib/encoding.md, stdlib/io.md, stdlib/strings.md -->

# CSV

RFC 4180, with the delimiter and quote character as parameters because real files
ignore the RFC.

CSV is a table of records, and a record is a struct. So the first API is the typed
one — it uses the same `Encode`/`Decode` derivation as JSON (`std.encoding`), so
anyone who has written `json.decode<T>()` already knows this module.

## Typed (the default)

<!-- test: skip -->
```rask
struct Sale {
    region: string
    units: u32
    revenue: f64
}

let sales = try csv.decode<Sale>(text)      // Vec<Sale>
let out = csv.encode(sales)                 // string, header row included
```

Column names come from field names; the header row maps columns to fields by name,
so column order in the file doesn't matter. Rename with the field annotation
`std.encoding` already defines.

## Streaming

<!-- test: skip -->
```rask
let file = try fs.open("sales.csv")
ensure file.close()

for row in csv.stream<Sale>(file) {
    let sale = try row
    process(sale)
}
```

`decode` reads it all; `stream` yields one record at a time and never holds more
than a row and one read's worth of input. Two names because they're different
shapes, not two spellings of one operation — a 4 GB file can't be a `Vec`.

A record that's bad on its own (`BadValue`, `ColumnCount`) comes out as an error
item and the stream carries on, so a loop can log a bad row and `continue`.
Anything else ends it: after a failed read, an unclosed quote or a header that
doesn't fit, there's no next record to find.

## Raw rows

<!-- test: skip -->
```rask
for row in try csv.rows(text) {      // Vec<Vec<string>>
    let first = row[0]
}
```

For files with no header, ragged rows, or columns you're discovering as you go.
`csv.write_rows(rows)` is the matching direction.

`rows` returns `Vec<Vec<string>> or CsvError`. It used to promise a bare
sequence of rows, which left nowhere for an unclosed quote to go, and the text
is already in memory, so a `Vec` costs nothing `decode` doesn't. Streaming is
`stream`'s job.

## Options

<!-- surface -->
```rask
extend csv {
    func decode<T: Decode>(text: string, delimiter: char = ',', quote: char = '"', headers: bool = true) -> Vec<T> or CsvError
    func encode<T: Encode>(records: Vec<T>, delimiter: char = ',', quote: char = '"', headers: bool = true) -> string
    func stream<T: Decode, R: Reader>(source: R, delimiter: char = ',', quote: char = '"', headers: bool = true) -> Sequence<T or CsvError>
    func rows(text: string, delimiter: char = ',', quote: char = '"') -> Vec<Vec<string>> or CsvError
    func write_rows(rows: Vec<Vec<string>>, delimiter: char = ',', quote: char = '"') -> string
}
```

The write side takes the same `delimiter` and `quote` as the read side. A file
read with `delimiter: ';'` should be writable back the same way, and without
them a European export was a one-way trip.

Semicolon-delimited European exports and tab-separated files are the same parser
with a different character — a parameter, not a `csv.Tsv` type or a builder chain
(`std.api/SD2`).

## Core Rules

| Rule | Description |
|------|-------------|
| **C1: Typed first** | `decode<T>`/`encode` are the primary API and use the derived `Encode`/`Decode` interfaces. `rows` exists for data that has no shape to decode into |
| **C2: Headers by name** | With `headers: true` (the default), columns map to fields by name and file column order is irrelevant. With `headers: false`, they map by position |
| **C3: Quoting is automatic on write** | A field containing the delimiter, a quote, or a newline is quoted; nothing else is. The caller never asks for quoting |
| **C4: Rows are validated against the header** | A row with a different column count than the header is an error, not a silently short record. Ragged files are what `rows` is for |
| **C5: Missing column is an error, empty cell is not** | A column the struct needs but the file lacks fails at the header. A `T?` field or one with `@default` doesn't need its column. An empty cell decodes normally — to `""`, `0`, `false`, or `none` for `T?` — because blank is what CSV writes for missing data |
| **C6: One decoder** | `decode<T>` and `encode<T>` go through the same derived codec as `json.decode<T>`/`json.encode`, so `@rename`, `@no_serialize` and `@default` mean the same thing in both formats. Cells are checked against their field's type first, so a bad cell is reported with its row and column. A field that isn't a string, number or bool is written as JSON in its cell |

## Errors

<!-- surface -->
```rask
enum CsvError {
    MissingColumn(name: string)
    DuplicateColumn(name: string)
    ColumnCount(row: usize, expected: usize, found: usize)
    BadValue(row: usize, column: string, reason: string)
    UnclosedQuote(row: usize)
    Io(IoError)
}
extend CsvError {
    func message(self) -> string
}
```

Every error about a record carries the row number. A parse failure on line 40,000
of a spreadsheet is useless without it. Rows count records from 1 with the header
as row 1, which is how a spreadsheet numbers them.

`DuplicateColumn` and `Io` are there because the rest of the spec needs them:
the edge cases make a repeated header name an error, and `stream` reads a
`Reader`, whose failures have to come out somewhere.

## Error Messages

```
ERROR [std.csv/C5]: CSV is missing a column this struct needs
   |
7  |  let sales = try csv.decode<Sale>(text)
   |                              ^^^^ `Sale.revenue` has no matching column

WHY: The header row is "region,units,total". Fields map to columns by
     name, and there is no "revenue" column.

FIX 1: Rename the field to match the file:

  struct Sale {
      @rename("total")
      revenue: f64
  }

FIX 2: Make it optional if the column is sometimes absent:

  revenue: f64?
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| CRLF line endings | — | Handled. `\r\n` and `\n` both end a row |
| Quoted field containing a newline | C3 | One field, spanning lines. Row numbers count records, not lines |
| Quote inside an unquoted field — `ab"c` | — | Kept as a literal character. So is text after a closing quote |
| Empty file, or header only | — | `decode` gives an empty `Vec` |
| `headers: false` | C2 | Fields take columns in declaration order; every row needs exactly that many |
| Cell out of range — `300` for a `u8` | C6 | `BadValue` naming the row and column |
| `""` inside a quoted field | C3 | An escaped quote |
| Trailing newline at end of file | — | Not an empty final row |
| Blank line mid-file | C4 | Skipped. It is not a row of one empty field |
| Byte order mark at the start | — | Stripped. Spreadsheet exports are full of them |
| Duplicate column names in the header | C2 | `DuplicateColumn` — which one a field means is unanswerable |
| Extra columns the struct doesn't have | C2 | Ignored |
| Non-UTF-8 bytes | — | `decode` and `rows` take a `string`, so it can't arise there. `stream` reads bytes and reports `BadValue` with an empty column for the row it was on, then stops. CSV has no encoding declaration; convert first |

---

## Appendix (non-normative)

### Rationale

**C1 (typed first):** the earlier sketch was a `Reader`/`Writer` class pair with
`row["name"]` string lookups — a transliteration of what CSV libraries look like in
languages that can't derive a decoder. Rask can (`std.encoding`), which is the whole
reason those interfaces exist. The guess test settles it: someone who has used
`json.decode<T>()` will type `csv.decode<Sale>(text)`, and getting a
`Vec<string>` with stringly-typed lookups instead would be the stdlib being wrong,
not the guess.

The typed API is also where the error messages live. `row["revenue"]` returning
`none` tells you nothing; failing at the header with "the file has `total`, your
struct wants `revenue`" tells you what to do.

**Why CSV is in the stdlib when TOML and YAML are not:** the README's out-of-scope
list rules out formats with opinions. CSV has none — it's a delimiter, a quote
character, and rows. The whole spec is one page and hasn't changed since 2005. It
also can't be avoided: CSV is what every spreadsheet, database and reporting tool
emits, which makes it a data-processing prerequisite rather than a format choice.

**C4 (ragged rows are an error):** libraries that pad short rows produce records
with silently-empty fields, and the bug surfaces later as a zero in a total. If a
file is genuinely ragged, `rows` hands you exactly what's there.

### See Also

- `std.encoding` — the `Encode`/`Decode` derivation this is built on
- `std.json` — same typed shape for a different format
- `std.io` — `stream` takes anything implementing `Reader`

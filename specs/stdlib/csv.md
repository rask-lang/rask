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
than a row. Two names because they're different shapes, not two spellings of one
operation — a 4 GB file can't be a `Vec`.

## Raw rows

<!-- test: skip -->
```rask
for row in csv.rows(text) {          // Iterator<Vec<string>>
    let first = row[0]
}
```

For files with no header, ragged rows, or columns you're discovering as you go.
`csv.write_rows(rows)` is the matching direction.

## Options

<!-- test: skip -->
```rask
csv.decode<T>(text: string, delimiter: char = ',', quote: char = '"', headers: bool = true)
csv.rows(text: string, delimiter: char = ',', quote: char = '"')
```

Semicolon-delimited European exports and tab-separated files are the same parser
with a different character — a parameter, not a `csv.Tsv` type or a builder chain
(`std.api/SD2`).

## Core Rules

| Rule | Description |
|------|-------------|
| **C1: Typed first** | `decode<T>`/`encode` are the primary API and use the derived `Encode`/`Decode` traits. `rows` exists for data that has no shape to decode into |
| **C2: Headers by name** | With `headers: true` (the default), columns map to fields by name and file column order is irrelevant. With `headers: false`, they map by position |
| **C3: Quoting is automatic on write** | A field containing the delimiter, a quote, or a newline is quoted; nothing else is. The caller never asks for quoting |
| **C4: Rows are validated against the header** | A row with a different column count than the header is an error, not a silently short record. Ragged files are what `rows` is for |
| **C5: Missing column is an error, empty cell is not** | A column the struct needs but the file lacks fails at the header. An empty cell decodes normally — to `""`, `0`, or `none` for `T?` — because blank is what CSV writes for missing data |

## Errors

<!-- test: parse -->
```rask
enum CsvError {
    MissingColumn(name: string)
    ColumnCount(row: usize, expected: usize, found: usize)
    BadValue(row: usize, column: string, reason: string)
    UnclosedQuote(row: usize)
}
```

Every error carries the row number. A parse failure on line 40,000 of a spreadsheet
is useless without it.

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
      @csv(name: "total")
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
| `""` inside a quoted field | C3 | An escaped quote |
| Trailing newline at end of file | — | Not an empty final row |
| Blank line mid-file | C4 | Skipped. It is not a row of one empty field |
| Byte order mark at the start | — | Stripped. Spreadsheet exports are full of them |
| Duplicate column names in the header | C2 | Error — which one a field means is unanswerable |
| Extra columns the struct doesn't have | C2 | Ignored |
| Non-UTF-8 bytes | — | `Utf8Error` from `string` construction. CSV has no encoding declaration; convert first |

---

## Appendix (non-normative)

### Rationale

**C1 (typed first):** the earlier sketch was a `Reader`/`Writer` class pair with
`row["name"]` string lookups — a transliteration of what CSV libraries look like in
languages that can't derive a decoder. Rask can (`std.encoding`), which is the whole
reason those traits exist. The guess test settles it: someone who has used
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

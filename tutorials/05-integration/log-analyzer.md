# Challenge 5.1: Log Analyzer

Write `loganalyzer.rk`. Read a log file where each line is:
```
[LEVEL] timestamp message
```

Report:
- Count per level (INFO, WARN, ERROR)
- All ERROR lines with line numbers
- The busiest 1-minute window (if timestamps are parseable)

Use `cli.args()` for the file path. Create a sample log file to test with.

## Sample Log File

Create `test.log`:
```
[INFO] 2024-01-15T10:00:01 Server started on port 8080
[INFO] 2024-01-15T10:00:02 Loading configuration
[WARN] 2024-01-15T10:00:03 Config key 'timeout' missing, using default
[INFO] 2024-01-15T10:00:15 Accepted connection from 192.168.1.10
[ERROR] 2024-01-15T10:00:16 Failed to authenticate user: invalid token
[INFO] 2024-01-15T10:00:20 Request: GET /api/users
[INFO] 2024-01-15T10:00:21 Request: POST /api/data
[WARN] 2024-01-15T10:00:22 Slow query: 850ms
[ERROR] 2024-01-15T10:00:30 Database connection lost
[INFO] 2024-01-15T10:00:31 Reconnecting to database
[INFO] 2024-01-15T10:00:32 Database reconnected
[ERROR] 2024-01-15T10:01:05 Out of memory: allocation of 1GB failed
```

## Design Questions

- How many structs/enums did you define?
- Did the string parsing feel adequate or did you want regex?
- Total line count — is it competitive with Python? With Go?
- How did error handling feel for the file I/O parts?

<details>
<summary>Hints</summary>

- Parse each line: split on `]` to get level, then split the rest on whitespace
- Use a `Map<string, i32>` for counting levels
- Collect ERROR lines in a `Vec` with their line numbers
- For the busiest minute: extract the minute prefix from timestamps, count occurrences
- `line.starts_with("[ERROR]")` is a simpler alternative to full parsing

</details>

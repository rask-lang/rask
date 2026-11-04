---
paths:
  - "**/*.rs"
  - "**/*.rk"
  - "**/*.c"
  - "**/*.h"
---

# Code comments

Neutral and direct, no "I":
- ✅ `// Skip to next declaration after error`
- ❌ `// I skip to the next declaration`

Remove obvious docs (`/// Get current token`), comments restating the code
(`// Check for X`), statement markers (`// While loop`), and AI explanations.

Keep section headers, non-obvious algorithms, important constraints, and the
"why" rather than the "what".

Tighten everything:
- ✅ `/// Record error, return if should continue`
- ❌ `/// Record an error and return a boolean indicating whether we should continue`

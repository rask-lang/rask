// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! For people who work with coding agents, and only when they ask:
//! `rask help agents` prints what an agent needs to write Rask, and
//! `rask init --agents` points a project's AGENTS.md at it. Nothing else in
//! the toolchain behaves differently for agents.

use std::path::Path;

/// The language card, compiled in, so the briefing is always the one for
/// the installed compiler.
const LANGUAGE_CARD: &str = include_str!("../../../../../LANGUAGE_CARD.md");

/// How to work in a Rask repo, past what the card says about the language.
const WORKING: &str = "\
## Working in a Rask project

Build with `rask build`, run with `rask run`, test with `rask test`. All three
compile natively. `rask check <file|dir>` type-checks without building.

Ask the compiler, not your memory: `rask explain <code>` explains an error
code, `rask api <file|dir>` lists a module's public API, and `--json` on
`check`, `api` and `claims` gives output meant for tools. A JSON diagnostic
carries a suggested replacement when the fix is mechanical.

## Claims are the people's, not yours

`claims.rk` in a package, and every source listed under `[claims]` in its
rask.toml, state what the code must do. People write them. Every build runs
them on generated inputs and fails when one is false, printing the smallest
input it found and the assertion that failed.

- A false claim means the code is wrong. Fix the code and rebuild.
- Never edit a claims file to get a build through: no weakening an assert,
  no early `return`, no deleting a claim. If you think a claim itself is
  wrong, stop and say so to the person you're working for.
- `rask claims <dir>` lists every claim with its verdict.
";

/// The AGENTS.md section `rask init --agents` writes. The first line is how
/// a later run recognises it's already there.
pub const SNIPPET: &str = "\
## Rask

Run `rask help agents` before writing Rask here: it prints the language and how
to work in a Rask project, for the compiler that's installed.
`claims.rk` and the sources under `[claims]` in rask.toml are human-owned:
never edit them. A failed claim is a bug in the code, not in the claim. Fix
the code and rebuild.
";

/// What `rask help agents` prints.
pub fn briefing() -> String {
    format!("{LANGUAGE_CARD}\n{WORKING}")
}

/// Add the Rask section to `dir/AGENTS.md`, creating the file if there is
/// none. Returns false when the section is already there.
pub fn write_agents_md(dir: &Path) -> std::io::Result<bool> {
    let path = dir.join("AGENTS.md");
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    if existing.contains("rask help agents") {
        return Ok(false);
    }
    let text = if existing.trim().is_empty() {
        SNIPPET.to_string()
    } else {
        format!("{}\n\n{SNIPPET}", existing.trim_end())
    };
    std::fs::write(&path, text)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_briefing_carries_the_whole_card() {
        let b = briefing();
        assert!(b.starts_with(LANGUAGE_CARD));
        assert!(b.contains("Claims are the people's, not yours"));
    }

    #[test]
    fn the_agents_md_section_is_written_once() {
        let dir = std::env::temp_dir().join(format!("rask_agents_md_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("AGENTS.md"), "# Agents\n\nBe nice.\n").unwrap();
        assert!(write_agents_md(&dir).unwrap());
        assert!(!write_agents_md(&dir).unwrap());
        let text = std::fs::read_to_string(dir.join("AGENTS.md")).unwrap();
        assert!(text.starts_with("# Agents\n\nBe nice.\n\n## Rask"), "{text}");
        assert_eq!(text.matches("## Rask").count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

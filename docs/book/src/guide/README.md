# Language Guide

The guide teaches the language one concept at a time, in the order you meet them, and says why each
rule is the way it is. It's being written chapter by chapter, so it has gaps.

Three other places answer different questions:

| You want | Go to |
|---|---|
| A rule, quickly, while writing code | [Language card](https://github.com/rask-lang/rask/blob/main/LANGUAGE_CARD.md) |
| A whole working program to copy from | [Example programs](../examples/README.md) |
| The exact, normative wording | [Specifications](../reference/specs-link.md) |

## Chapters

- [Passing Values](passing-values.md): borrow, `mutate`, `take`, and why only one of them needs a
  marker at the call site

## What a chapter owes you

The specs are normative and say what a rule is. A chapter's job is the part the spec tables leave
out: why it's that way, and what it would cost to be otherwise.

Every chapter's code is pulled out of a program CI runs on both backends, and every compile error is
a pinned recording of what `rask check` actually prints. So a chapter can't quietly drift from the
compiler, and improving an error message shows up as a diff on the page that teaches it. The rules
for writing one are in [how this book is built](../contributing/how-this-book-is-built.md).

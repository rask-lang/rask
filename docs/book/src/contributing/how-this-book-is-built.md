# How this book is built

Rules for anyone adding a page. They exist because a book is a second copy of the
language, and the usual fate of a second copy is to drift until it teaches a version
of the language nobody ships.

## What a page may contain

Prose, and code that something else already verifies. Nothing else.

| You want to show | Write it as |
|---|---|
| A few lines from a real program | `\{{#include ../path/prog.rk:anchor}}` |
| A small self-contained snippet | an inline block with `<!-- test: compile -->` or `<!-- test: run \| expected output -->` |
| What the compiler says when you get it wrong | `\{{#include ../../errors/<chapter>/<case>.out}}` |
| What a program prints | `\{{#include ../../../../tests/golden/<name>.out}}` |

`<!-- test: parse -->` does not count as verified anywhere in the book. Parsing proves the
syntax is current; syntax is not what rots. The front-page snippet passed `test: parse`
while calling `fs.open`, which the real grep program doesn't use, and missing both its
imports, so a reader copying it got two errors on page one. `compile` type-checks;
`run | expected` runs it and matches the output. Use one of those.

CI runs the markers in its "Docs snippets parse" step, which has covered `docs/` for a
while. What it can't tell you is whether a block has a marker at all, or whether the one
it has is strong enough, which is what `tests/book_gate.sh` is for. It also checks that every include resolves: the file is
present *and* the named anchor really is in it, because mdBook renders a missing anchor as
nothing at all and says so quietly.

## Errors are content

Chapters teach with real compiler output. Each case is a program under
`docs/book/errors/<chapter>/` that must **not** compile, with its `rask check` output
pinned beside it. The gate re-renders and diffs on every run, so improving a diagnostic
shows up as a diff on the page that teaches it, which is the review you want. A case
that starts compiling is a hard failure, not a stale golden: the page is claiming a
rejection the compiler no longer makes.

Regenerate after a deliberate diagnostics change:

```bash
tests/book_gate.sh --update
```

## How to explain it

The machine is the metaphor. Describe what actually happens in memory, in plain words,
and don't build a story on top of it.

Lending is the cautionary case. In a library the book leaves the shelf; in Rask the
caller goes on reading the value for the whole call. A reader who builds the library
model predicts the wrong thing at their first `mutate` call — and learned it here.

Rask can afford the literal route where most languages can't. No lifetime annotations,
no reference type, no effects: every rule is a claim about bytes and where they sit. So
make the claim.

**One noun carries it: storage.** A place in memory holding a value. A name reaches
storage, a move hands it over, a borrow reads storage the function doesn't own, a copy
makes a second one. Define it once and don't introduce a parallel vocabulary three
chapters later.

**Idea first, word second.** Build the model in plain language, then attach the term in
one sentence, as a hook for the error messages:

> A function that takes a value without `take` reads the caller's storage where it sits
> — it doesn't get a copy, and it doesn't get to keep it. The compiler calls this
> **borrowing**, and you'll see the word in error messages.

Jargon is a label for something the reader already has. Arriving before the idea is what
makes a language feel like it was written for people who already know it.

**Spend a metaphor only where the literal thing isn't observable.** In Rask that's
almost nowhere, which is the design working rather than a style choice. Every spend
should feel like a defeat.

The test for a paragraph: could a reader predict the next error message from what it
just told them? If they'd have to re-derive it from a metaphor first, the paragraph is
doing the wrong job.

## The book teaches the ruling, not just the rule

The specs are normative and say what the rule is. A chapter's job is the part the spec
tables leave out: why it's that way, and what it would cost to be otherwise. "The marker
is required" is a spec line. "A misread move is caught for you and a misread mutation
isn't, so the one that can't be caught is the one you write down" is a chapter.

[specs/RULINGS.md](https://github.com/rask-lang/rask/blob/main/specs/RULINGS.md) is where those arguments come from. If a
chapter can't say why, it's restating the spec and should link to it instead.

## Size is capped by the language, not by the author

[specs/DAY_ONE.md](https://github.com/rask-lang/rask/blob/main/specs/DAY_ONE.md) holds the reading set: the concepts you need
to read someone else's Rask, which must fit one page. The guide inherits that budget:
roughly one chapter per Day-One concept, and no chapter for something that isn't in the
language yet.

This is the answer to books that grow to a thousand pages: the guide can't outgrow the
language, because its table of contents is the language's own list. Anything deeper goes
to `specs/`, which is allowed to be long.

## What to do when the language changes

Nothing, usually. That's the point: included code moves with its program, error
renderings are regenerated by the gate, program output comes from the goldens. The
prose only needs touching when a *decision* changes, which is rare and deliberate.

If you find yourself keeping a page in step by hand, the page is built wrong.

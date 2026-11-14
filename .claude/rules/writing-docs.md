---
paths:
  - "**/*.md"
  - "docs/**"
---

# Writing docs

Sound like a developer with a vision, not a committee or AI. Natural flow over
perfect grammar. Don't be TOO consistent.

**Use "I" for design choices:**
- ✅ "I chose handles over pointers—indirection cost is explicit"
- ❌ "It was decided that handles should be used"

**Keep technical sections neutral:**
- ✅ "References cannot outlive their lexical scope"
- ❌ "I make sure references cannot outlive scope"

**Be direct about tradeoffs:**
- ✅ "This means more `.clone()` calls. I think that's better than lifetime annotations"
- ❌ "While this may result in additional clones, it provides benefits..."

**Remove filler:** "It should be noted", "In order to", "With regard to". Em-dashes
too: a full stop or a colon, nearly always.

**Say the thing, don't circle it.** Every sentence owes the reader a fact they
didn't have. Delete it and see what's lost; if nothing, it was noise. All of
these came off one pass over the website:

- ❌ Restating the code in prose underneath it
- ❌ Explaining a default by calling it the default
- ❌ Three restatements of one abstraction in a row
- ❌ Throat-clearing before the content: "Not a lint, not a guideline:"
- ❌ A wink where an instruction belongs: "there's a box below if you want to disagree"
- ❌ Narrating what the page already shows: "this runs the real interpreter", above a Run button

**Start where the reader already is, one concept at a time.** The parameter
chapter opened on ownership and left "small values just copy" for the end, so a
reader carried ownership anxiety through four sections before finding out it
mostly didn't apply. Lead with the half they already understand. Same reason a
page says what you get before what it costs.

**Natural language OK:** contractions, slight grammar quirks, Scandinavian English flow.

## Check what the page claims

**A sentence about Rask is a claim. Check it like code.** An unchecked one ships
sounding exactly as sure as a checked one. Three got onto the website in an
afternoon: "pre-0.1" with v0.2.0 released, "Rask doesn't offer pointer-level
control" when `unsafe` gives raw pointers, and a headline that described Go.
Version and status against `git tag` and the releases; what the language has
against `specs/`; a claim that Rask *differs* names what the other language
actually does. If the sentence is equally true of Go, it isn't about Rask.

**Verify the path, not only the artifact.** The front page's code came out
unstyled while the deployed file was byte-perfect: the URL was stale. Ask how
the thing reaches a reader, and check that.

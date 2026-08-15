<!-- id: analysis.fourth-option-naming -->
<!-- status: exploration -->
<!-- summary: Naming the two types — why Graph and Edge are both wrong, and what to call them instead -->
<!-- depends: analysis/fourth-option.md, stdlib/api-design.md -->

# Naming: Not `Graph`, Not `Edge`

Both working names describe the *topology a program might build*, not the
*job the type does*. That's backwards, and the api-design guess test
(`std.stdlib/SD*`) catches it.

## `Graph<T>` is wrong for most of its uses

Look at what actually gets declared:

<!-- test: skip -->
```rask
tasks: Graph<Task>          // a task store. Not a graph
entities: Graph<Entity>     // a game's entities. Not a graph
lines: Graph<Line>          // a text buffer. Not a graph
users: Graph<User>          // a user table. Not a graph
```

A user with a flat list of tasks has to declare a `Graph` and doesn't have a
graph. The word describes a shape their data *might* take, and usually
doesn't. That's the wrong end of the telescope: name the container for what
it is — a place where many instances of a type live with stable identity —
not for one possible arrangement of its contents.

**Recommendation: keep `Pool<T>`.**

The container's job hasn't actually changed. It's still "many things of one
type live here, individually addressable, individually removable." What
changed is *how you refer to its contents* — handles became something better.
Keeping the name means:

- The concept transfers intact; nobody relearns what a pool is.
- The change lands as "pools hand out links now, not handles" — one idea,
  not a new vocabulary.
- Migration is a find-and-replace on the reference type, not on every
  container declaration in every program.
- It doesn't lie to the 80% of users whose pool holds a flat collection.

The one argument for renaming — that the container now maintains referential
integrity, so it "knows about relationships" — doesn't require the name to
say so. `Vec` doesn't announce that it reallocates.

## `Edge<T>` only makes sense next to `Graph<T>`

Drop `Graph` and `Edge` loses its anchor: an edge is a graph-theory term, and
it's jargon for what is, plainly, a reference to something in a pool.

Options weighed:

| Name | For | Against |
|---|---|---|
| `Edge<T>` | precise if you think in graphs | jargon; meaningless without `Graph`; pairs badly with `Pool` |
| `Ref<T>` | it *is* the language's one storable reference | collides head-on with "Rask has no storable references" — the sentence stops being true and starts needing an asterisk |
| `Ptr<T>` | short | implies raw memory; `&raw` already owns that space |
| **`Link<T>`** | plain English, no jargon; the verb is already right — it links, and when the target dies it *unlinks* | slightly soft-sounding |

**Recommendation: `Link<T>`.**

<!-- test: skip -->
```rask
struct Task {
    title: string
    blocked_by: Link<Task>?
    deps: Vec<Link<Task>>
}

struct Store {
    tasks: Pool<Task>
    by_id: Map<TaskId, Link<Task>>
}
```

Read it aloud: "blocked_by is a link to a task, maybe." "deps is a vector of
links to tasks." Both land without a glossary, and the behaviour has a verb
that matches — the target dies, the link unlinks.

Against `Handle<Task>?` at 13 characters, `Link<Task>?` is 11 and carries
meaning instead of implying a ticket you must redeem.

## Is the `?` pulling its weight?

Yes. Required and optional links both exist (required ones are constructible
inside a batch), and the distinction is real at use sites: a required link
never needs unwrapping, an optional one always does. So `?` marks a genuine
difference rather than decorating every declaration.

And it keeps the read path unified with everything else optional in the
language — `link? as t`, `link?.title`, `link?.title ?? "none"` all work
because it *is* an optional, not because links got their own operators.

## The more radical option, noted and not taken

Declare node-ness on the type, then drop the wrapper entirely:

<!-- test: skip -->
```rask
node struct Task { ... }

struct Something {
    blocked_by: Task?        // unambiguous: Tasks live in pools, so this is a reference
    deps: Vec<Task>
}
```

Cleanest possible use sites, and it removes a generic wrapper from every
declaration. Two reasons not to take it now: it hides the cost (a field
that looks like a plain value carries a back-pointer), and it splits struct
declarations into two kinds, which is a much larger language change than
adding one type. Worth revisiting if `Link<T>` proves noisy in real schemas.

## The container: still iterating

`Link<T>` is settled. The container is not, and the reason surfaced from a
reader's first reaction to `Pool<T>` + `Link<T>`: *"it's a pool of links?"*

That's the real criterion. **The two names have to form a pair that teaches
the model.** `Graph`/`Edge` paired beautifully and one half was wrong.
`Pool`/`Link` has both halves defensible and no relationship between them —
"pool" says nothing about why the things inside can be linked to, so the
reader has to ask.

Working candidates, judged as pairs:

| Pair | Teaches | Against |
|---|---|---|
| `Pool<T>` + `Link<T>` | nothing — two unrelated words | familiar; zero migration on container declarations; but carries object-pool baggage (recycling, reuse) that was never what this is |
| **`Table<T>` + `Link<T>`** | the actual model — this *is* `ON DELETE SET NULL`, so the DB analogy is a teaching tool, not a metaphor | "table" means hash-map in Lua and rows-and-columns to everyone else; game devs may find `Table<Entity>` odd |
| `Registry<T>` + `Link<T>` | things register and get identity | verbose; `Registry<Entity>` is a mouthful in hot-path code |
| `Store<T>` + `Link<T>` | plain and honest | collides with real programs — the flagship's own struct is named `Store` |

**`Table<T>` is out.** It isn't a table — no rows, no columns, no schema in
the SQL sense, and "table" already means hash-map to a large audience. The
model *resembles* a database; the container isn't one.

### The cold-read test

Same three declarations under each candidate. A name has to survive all
three, because the container is general — it holds whatever a program has
many of.

| Candidate | `<Task>` | `<Entity>` | `<Line>` | Verdict |
|---|---|---|---|---|
| `Pool` | fine | fine | fine | survives everywhere, teaches nothing, carries object-pool (recycling) baggage |
| `Roster` | good — a roster of tasks | good | odd | members with identity that join and leave; the meaning is exactly right |
| `Colony` | odd | good | odd | real prior art (`plf::colony` is a stable-reference container), but reads whimsical |
| `Web` | ok | odd | odd | pairs perfectly with `Link` — but Rask has `net`/`http`, so "web" is loaded |
| `Ledger` | good | odd | odd | entries with identity; implies append-only accounting |
| `Nest`, `Cohort`, `Zone` | odd | ok | odd | no meaning carried; just unfamiliar |

Nothing survives all three cleanly except `Pool`, which is the finding.

### What that suggests

The candidates that *teach* (`Roster`, `Ledger`, `Web`, `Colony`) all teach by
analogy to one domain, and then read wrong in the other two. That's not bad
luck — it's what happens when a general container gets a specific name. The
same objection that killed `Graph` and `Table` kills each of them at the
schema where their analogy doesn't hold.

So the pair criterion needs qualifying: **the two names should not fight each
other, but only the reference has to teach.** `Link<T>` carries the whole
lesson — it links, it unlinks when the target dies. The container just needs
to say "many of these live here" without lying, and stay out of the way.

By that standard the choice is between the neutral options:

- **`Pool<T>`** — already learned, already in every existing program, honest
  about the job. Its only sin is being uninformative, which is now the
  requirement rather than the flaw. The object-pool connotation is worth one
  sentence in the docs.
- **`Roster<T>`** — the most accurate neutral word available: current
  members, by identity, joining and leaving. Costs a rename everywhere and
  reads oddly for non-membership data.

Current lean: **`Pool<T>` + `Link<T>`**, arrived at from the opposite
direction than before — not because the container's name should be familiar,
but because every evocative alternative turned out to overfit one domain.

Still genuinely open. The strongest argument against `Pool` is that it names
the old model's ticket-and-recycle mental image, and someone learning
`Link<T>` on top of `Pool<T>` may import the wrong intuitions.

## Recommendation so far

`Link<T>?` for the reference — decided. Container leaning `Pool<T>`, on the
finding that every evocative candidate overfits one domain and reads wrong in
the others. `Roster<T>` is the one alternative still worth a second look.

Everything else in the exploration is unaffected — this is spelling, and the
semantics were settled first on purpose.

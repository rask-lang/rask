# Projects

These projects build on one idea: **if a computation is deterministic, anyone can verify it by running it again.**

That's the foundation. Everything else follows from it.

If I can verify a computation, I can enforce rules mechanically. "Nothing appeared from nothing. Nothing was copied. Everything has an origin." Conservation laws — like physics, not policy.

If rules are enforced mechanically, two parties don't need a third party to trust each other. You show me the proof, I run it myself, done. Trust builds up from pairs, not down from authority.

---

## The pieces

**[Raido](raido/)** — A small deterministic VM. Same input, always the same output. No floating point, no randomness, no I/O. The host provides everything. Because it's deterministic, anyone can re-run a computation and verify the result.

**[Leden](leden/)** — A networking protocol built on capabilities. Not "you have access to everything" or "you have access to nothing" — but "you can do exactly this, with exactly this thing, and I can revoke it whenever I want." Fine-grained, delegatable authority between endpoints.

**[Allgard](allgard/)** — A federation model. Independent domains cooperate without central control. Each domain is sovereign — it mints its own assets, enforces its own rules. When two domains trade, both sides can verify the transaction was legitimate. No global consensus needed.

**[GDL](gdl/)** — A schema for describing spatial environments. Regions, entities, things you can interact with. The content format that travels over the protocol.

**[Midgard](midgard/)** — A virtual world that puts it all together. Independent servers, each running their own world. A sword can't be copied. A currency can't be inflated. A character can travel between servers and bring their things — because both sides can verify everything checks out. It's the most intuitive example, but the model isn't about games.

---

## How they connect

Raido makes verification possible. Leden controls who can do what. Allgard defines the rules for cooperation. GDL describes the content. Midgard shows it working.

The point isn't any single piece. It's that deterministic computation, capability-based access, and conservation laws together give you decentralised cooperation — without blockchains, without central authorities, without having to trust anyone.

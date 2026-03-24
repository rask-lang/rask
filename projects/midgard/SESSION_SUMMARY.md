# Session Summary: World Description Protocol

## Context

Reviewed the full Midgard stack (Raido, Allgard, Leden) and stress-tested the design. The pushback surfaced a missing layer.

## What Holds Up

- **Raido** handles deterministic game logic (crafting, combat, NPC behavior). Scripts are content-addressed, verifiable, fuel-limited.
- **Allgard** handles federated ownership. Conservation laws prevent duplication, inflation, conjuring. Bilateral trust, not global consensus.
- **Leden** handles capability-based networking. Sessions, object references, gossip discovery.
- **Domain crossing** is well-designed — pre-staging, compatibility reports, player consent, escrow on failure. Honest about friction.

## What's Missing: World Description Protocol

The stack has logic (Raido), trust (Allgard), and transport (Leden). It does NOT have a way to tell a client **what the world looks like and how to interact with it**.

This is the missing layer between domain state and player experience.

### The Insight

The client is not a game engine. It's a **renderer for a world description protocol** — the same way a browser renders HTML. Domains don't send rendering code. They send structured descriptions of what exists. The client decides how to display it.

### Why Photorealism Doesn't Matter

- Minecraft proved low-fi + creativity beats photorealism
- MUDs proved text alone works
- Roblox proved janky UGC at scale beats polished single-studio content
- The protocol carries *descriptions*, not meshes

### Progressive Fidelity

Same domain data, different clients:
- **Text client** sees room descriptions, item lists, action menus
- **2D client** sees a tile map with sprites
- **3D client** sees a low-poly scene with common assets

The description format must support all three without being designed exclusively for any one.

### Asset Strategy

- **Common asset vocabulary.** Client ships with base primitives (terrain types, basic shapes, materials). Domains describe scenes using these. Like how browsers ship with layout engines and form controls.
- **GenAI gap-filling.** Domain says "oak tree, 3m tall" — client generates or retrieves locally. Protocol carries descriptions, not geometry.
- **Domain-specific assets.** Domains CAN provide custom assets for download. Loading on domain entry is normal and expected.

### The Design Problem

This is Midgard's "HTML" — the scene description format:
- **Too rigid** → domains can't express anything interesting
- **Too flexible** → clients can't render consistently
- **Interaction model** → how does the player interact with domain-specific objects? A sword and a wrench need different affordances. The protocol needs "what can you do with this," not just "what does this look like."

## Open Questions for Next Session

1. **What does the world description format look like?** Scene graph? Entity list with properties? Spatial regions with descriptions? Some hybrid?
2. **How do interactions work?** Domain advertises affordances per object type? Client maps affordances to input?
3. **How does progressive fidelity work in practice?** Does the domain send one description that clients interpret differently, or multiple representations?
4. **Where does this sit in the stack?** Is it a Leden content type? A Raido output format? Its own layer?
5. **What's the minimum viable version?** Text + basic spatial layout? That would let us test domain crossing and interaction without solving rendering.

## Criticism That Still Stands

- **Scope is enormous.** Language + VM + federation + protocol + world description + client. Each is multi-year.
- **Federation's track record is poor.** Email → Gmail dominates. XMPP → WhatsApp won. Mastodon → niche. Centralized platforms win on UX. The counter-argument is that games are different (domain sovereignty has real value), but this is unproven.
- **Conservation laws under adversarial pressure.** Untested. Sybil attacks (10,000 fake domains for reputation), audit gossip minimum-honest-domain thresholds — these need analysis.
- **Domain hosting economics.** "Anyone can host" still means someone pays. Popular domains face scaling costs. This pushes toward centralization.

## Suggested Next Steps

1. Draft the World Description Protocol spec — start with the entity/scene model
2. Define the interaction/affordance model — what can you do with things
3. Build the simplest possible client — text-based, renders descriptions, sends actions
4. Get two domains talking over Leden with one object transferring and conservation laws verified — that's the real proof of concept

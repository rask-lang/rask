# Session Summary: World Description Protocol → GDL

## Context

Reviewed the full Midgard stack (Raido, Allgard, Leden) and stress-tested the design. The pushback surfaced a missing layer — how does a client know what the world looks like and how to interact with it?

## Resolution

The missing layer is [GDL — Gard Description Language](../gdl/). GDL is to Leden what HTML is to HTTP. Domains send structured descriptions (regions, entities, affordances, appearance). Clients render them however they want — text, 2D, 3D.

See [GDL.md](../gdl/GDL.md) for the full spec and [GDL-style.md](../gdl/GDL-style.md) for the style system.

### Key Design Decisions (from the original session)

- **The client is a renderer, not a game engine.** Like a browser rendering HTML. Domains send descriptions, not rendering code.
- **Progressive fidelity.** Same GDL data, different clients: text sees room descriptions, 2D sees tile maps, 3D sees scenes.
- **Photorealism doesn't matter.** Minecraft, MUDs, Roblox all prove this. The protocol carries descriptions, not meshes.
- **Common asset vocabulary.** Clients ship with base primitives. GenAI gap-fills for novel descriptions. Domains can provide custom assets.

## Criticism That Still Stands

- **Scope is enormous.** Language + VM + federation + protocol + content schema + client. Each is multi-year.
- **Federation's track record is poor.** Email → Gmail dominates. XMPP → WhatsApp won. Mastodon → niche. Centralized platforms win on UX. The counter-argument is that games are different (domain sovereignty has real value), but this is unproven.
- **Conservation laws under adversarial pressure.** Untested. Sybil attacks (10,000 fake domains for reputation), audit gossip minimum-honest-domain thresholds — these need analysis.
- **Domain hosting economics.** "Anyone can host" still means someone pays. Popular domains face scaling costs. This pushes toward centralization.

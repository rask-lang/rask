# GDL — Gard Description Language

Content schema for describing gards (interactive spatial environments) over [Leden](../leden/).

GDL is to Leden what HTML is to HTTP. Leden handles transport, capabilities, and observation. GDL defines what's inside the payloads — regions, entities, affordances, appearance, spatial layers, input streams.

## Specs

| Spec | What |
|------|------|
| [GDL.md](GDL.md) | Core schema — regions, entities, affordances, appearance, bonds, panels, events |
| [GDL-extensions.md](GDL-extensions.md) | Optional extensions — streams, spatial layers, physics, nested spaces, immersive |
| [GDL-style.md](GDL-style.md) | Style system — design tokens, structured hints, CSS stylesheets |

## Stack

| Layer | Project |
|-------|---------|
| VM / scripting | [Raido](../raido/) |
| Federation / trust | [Allgard](../allgard/) |
| Transport / capabilities | [Leden](../leden/) |
| **Content / style** | **GDL** |
| Example application | [Midgard](../midgard/) |

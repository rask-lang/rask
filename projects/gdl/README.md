# GDL — Gard Description Language

Content schema for describing gards (interactive spatial environments) over [Leden](../leden/).

GDL is to Leden what HTML is to HTTP. Leden handles transport, capabilities, and observation. GDL defines what's inside the payloads — regions, entities, affordances, appearance, spatial layers, input streams.

## Specs

| Spec | What |
|------|------|
| [GDL.md](GDL.md) | Content schema — regions, entities, affordances, appearance, panels, spatial layers, input streams, physics |
| [GDL-style.md](GDL-style.md) | Style system — design tokens, structured hints, CSS stylesheets. GDL's CSS. |

## Stack

| Layer | Project |
|-------|---------|
| VM / scripting | [Raido](../raido/) |
| Federation / trust | [Allgard](../allgard/) |
| Transport / capabilities | [Leden](../leden/) |
| **Content / style** | **GDL** |
| Example application | [Midgard](../midgard/) |

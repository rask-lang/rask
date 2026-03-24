# Midgard

Virtual world architecture. A concrete example of [Raido](../raido/), [Allgard](../allgard/), and [Leden](../leden/) working together.

Midgard is an application — it uses the infrastructure projects, it doesn't define them. For the federation model, see [Allgard](../allgard/README.md). For capabilities and protocol mapping, see [OCAP.md](OCAP.md).

## What Midgard Adds

Game-specific concerns on top of the federation model:

- **Game object types** — swords, characters, regions, currency. Concrete types with game semantics mapped to Allgard Objects.
- **Lockstep simulation** — deterministic lockstep for small groups (2-16 participants). Raido's fixed-point arithmetic and seedable PRNG guarantee bitwise-identical results across machines.
- **UGC sandboxing** — entity scripts, modding, NPC AI via Raido. Scripts get only the references the host hands them. Fuel-limited.
- **Verifiable crafting** — crafting recipes, damage formulas, and economic transforms are Raido scripts. Cross-domain crafting is [verifiable](../allgard/README.md#verifiable-transforms) — the receiving domain re-executes the script to confirm the result.
- **Cross-domain rate limiting** — Conservation Law 5 is per-domain. Coordinated abuse from multiple domains needs application-level policy.

## Designed Entropy

Value sinks prevent inflation (Conservation Law 3). Without them, supply only grows.

| Sink | Mechanism |
|------|-----------|
| **Crafting loss** | 3 iron bars → 1 sword, not 3 ↔ 1. |
| **Repair costs** | Equipment degrades without upkeep. |
| **Item decay** | Consumables, buffs, temporary enchantments expire. |
| **Transaction fees** | Cross-domain transfers cost something. Bounds spam. |
| **Training costs** | Learning abilities consumes resources. |

Specific rates are domain policy — casual servers want low decay, hardcore servers want high. Conservation Law 3 just requires sinks are declared, not hidden.

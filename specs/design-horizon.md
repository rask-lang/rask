# Design Horizon

Which future is Rask built for? The honest worry that prompted this note: am I building a language for 2010, not 2030?

The way to build for 2030 is *not* to collect 2030's buzzwords — because the features that read as "advanced" aren't from 2030. Higher-kinded types are Haskell, 1990. Dependent types are Martin-Löf in the 70s, Coq in '89. Algebraic effects are late-90s research. Bolting them on wouldn't make Rask feel like the future; it'd make it cosplay 1995 grad-school PL. The frontier you're afraid of missing is mostly the *old* frontier.

## What actually dates a language

Not its feature list — what it aimed at. Rust aimed at a durable problem (memory safety without GC). Go aimed at a problem (concurrency and build speed at scale). Both feel current 15 years on. The languages that chased the era's fashion dated fastest, because a fashion expires and a problem doesn't. "Build for 2030" is the wrong frame. Build for a problem that's still real in 2030, and for whoever is *reading the code* by then.

## The bet Rask already makes

By 2030 most code is written by a machine and reviewed by a human. Rask already bets the whole design on this — it's why conformance is declared and not inferred, why analysis is function-local, why errors are loud and intent is explicit (principles 5, 8, 9 in [CORE_DESIGN.md](CORE_DESIGN.md)). A language tuned for local reasoning and self-describing signatures is a language tuned for *LLM-generates, human-reviews*.

Frontier abstraction is the anti-bet. Undecidable inference and proof obligations are exactly what models are worst at, and abstraction towers are exactly what a human reviewer can't check at a glance. So the restraint isn't nostalgia — it's the most forward-facing decision in the repo, and every frontier type feature would degrade it.

## The test

For any feature you're unsure about, ask: does omitting it **block a class of programs** people will need, or does it just **deny an abstraction style**?

HKT denies a style — every program stays writable, only with more per-container code. That's not a horizon risk. Run the same test on what Rask *doesn't* discuss and the real exposure shows up:

- **Heterogeneous / data-parallel hardware** — GPUs, accelerators, wide many-core. That's a *class of programs*, and it's growing toward 2030. Rask has no story for "run this across ten thousand lanes." This is the actual dated-shaped gap, worth watching far more than any type feature.
- **Federation, capabilities, local-first** — the `projects/` layer (Leden, Allgard) is already *ahead* of the fashion. Rask is early where it doesn't worry.

Type theory is where Rask is fine. Hardware parallelism is where it might fall behind. Distribution is where it's ahead. The worry, inverted.

## The insurance

Restraint only ages well if a "no" leaves a door. For each frontier feature Rask declines, the healthy pattern is a *stated* smaller-version or escape hatch, so a real future need can be met without a breaking redesign. Associated types (deferred, not rejected — see [types/generics.md](types/generics.md)) is the model: "not yet, here's the `*`-level version." A flat "never" with no path back is how a language locks itself into its birth year. Keep the deferred doors real and championed.

## The failure mode

The risk to a language like this was never too few features. It's blinking — feature-anxiety turning a coherent thesis into a grab-bag. What reads as "of its time" in the good way is coherence around a thesis the era rewards. Rask's — invisible safety, local reasoning, built for the machine-writes/human-reviews world — is a better 2030 bet than any kind system. Hold the line; prove it under load.

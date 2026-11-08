<!-- id: verdandi.games -->
<!-- status: notes -->
<!-- summary: What makes the target games good, and what that decides for a game on Verdandi -->

# What the games get right

Notes from thinking about my own taste before building anything. Evil Genius,
RCT2, AoE2, Red Alert 2, HoMM3, SimCity, The Sims, Spore, then Dwarf Fortress,
Minecraft, RimWorld, Factorio, Satisfactory, Prison Architect, They Are Billions.

## The common thread

You build a machine out of agents and watch it run without you. The pleasure is
the gap between what you designed and what happens. The "feeling of potential"
is knowing the system has more states than you've seen. The RTS half of the list
is the same loop compressed, with an opponent as the perturbation instead of a
raid or a fire. So the thread isn't emergence as such. It's a legible machine
that occasionally surprises you.

## Depth has to surface

Games aren't shallow because nobody thought of deeper rules. Depth only pays if
the player can perceive it, and surfacing it is the expensive part. Dwarf
Fortress has more depth than anything on the list and most of it is spent on
nobody. RimWorld computes far less and keeps nearly all of it visible ("Bob is
sad because he slept on the floor, and that's why he punched Alice"), and it
tells better stories to more people.

Rule: a sim variable earns its place only if it changes something the player
can see or act on.

## The 90s feel

In RA2 and AoE2 the sim was the picture. Every sprite was a state, the tile grid
made distance and cost literal, and input landed on the next tick with nothing
in between. Modern games smooth exactly those seams (animation blending, camera,
particles), and smoothing cause and effect makes a system feel mushy even when
it's deeper. Factorio and They Are Billions feel old because they kept the
crispness: fixed tick, chunky state, readable sprites, nothing decorative.

Rule: nothing on screen that isn't a variable. It's the same constraint
determinism wants.

## The start is the best part

Decision density peaks in the first hour. You have nothing, so every choice is
forced by the seed and the terrain, and every discovery changes the plan. Later
you have slack, the plan is known, the sim stops surprising, and what's left is
scale and maintenance. Most Factorio players never launch the rocket. RimWorld
players restart constantly. Nobody has fixed the mid-game, so I'd rather design
for restarting than try.

Decisions that follow:

- Runs are mortal and short (hours), and they end in a victory or a collapse,
  never a plateau.
- The design budget goes to the opening: a map whose scarcity sets the first
  problem, a short visible ladder of unlocks, feedback in seconds, an early
  deadline (first winter, first raid).
- The world persists across runs, as Dwarf Fortress does. The ruin, the
  survivors and the history carry forward, so a restart is a chapter, not a
  reset. The start is possibility, the end is the story, and a run that ends is
  a run that gets told.
- In co-op a new run is a new scenario for the group. Nobody babysits a
  200-hour colony.

## The mid-game is a sequence of starts

Automation alone doesn't fix it. Satisfactory automates as well as Factorio
and plateaus harder; RimWorld barely automates and holds people for hundreds
of hours. What works is that each tier removes one kind of attention demand
and replaces it with a different kind at a larger unit of decision. Factorio's
ladder (hand-feeding, belts, trains, bots, megabase ratios) makes each rung's
problem vanish and hands you one you've never seen, and the unit you reason
about changes with it: machines, lines, stations, the network. The feeling of
potential comes back every time the decision unit changes altitude.

Two conditions:

- Automation is built by the player out of sim parts (an inserter, a
  stockpile, a manager's workshop), never switched on in a menu. Then it's
  itself a machine you watch run, and it can break.
- It leaks. Bots run out of power, outposts get raided, routes jam. Surprises
  survive the tier change, so the system never reads as solved.

Digital circuits are the sharper model. A gate is built from transistors and
exposes a smaller interface than they do, and once you have gates you never
address a transistor again. That's composition, and it's what Dwarf Fortress
lacks: it has tier-shaped things (stockpiles, the manager, squads, burrows)
but none is built from the others and none hides the one below, so a hundred
dwarves is seven dwarves times fourteen with the same menus.

Where the analogy breaks is the sealed interface. A circuit wants no leaks; a
game wants some. The way to get both is to abstract the controls and the view
and never the sim. The player commands crews, districts and settlements and
the screen summarises at that altitude, but every worker underneath keeps
their own needs and the sim never changes resolution. A leak is then a tier-0
event showing up at tier 2: one worker snaps, the crew's output drops, with
the reason attached. Factorio works this way (a blueprint is a control
abstraction, the sim still runs every inserter). Paradox abstracts the sim
itself (pops as numbers), which scales and reads well, but nothing underneath
can misbehave on its own, so the stories thin out. DF is the third corner,
full sim and no control abstraction, and drowns at a hundred.

The lower tier never goes away, it stops being necessary. You can still probe
a transistor; the gate hides it by default. Same in play: at 200 pop you still
click one villager when a raid hits, and you hand-place a turret when the
biters breach. That's the crisis loop. Steady state lives at the top tier, a
leak pulls you down a level to fix it by hand, and the fix is often building
the automation that would have caught it, which is the next tier up.

So tiers of abstraction are a design object from day one. Each tier is built
from the tier below, is the default command surface but not the only one, and
the sim resolution stays at the bottom. Name the unit of command at each stage
(a worker, a crew, a district, a settlement) and what the player stops needing
to touch when the next unit appears.

## Mechanisms to build first

Four, and the rest is layers on top:

- Agents with needs and a scheduler that picks the next job from them. Cheap,
  and where every story in DF and RimWorld comes from.
- Spatial logistics. Things physically move and distance is cost, so layout is
  a decision instead of a menu.
- Resource loops with sinks, so an economy doesn't flatline.
- Scheduled perturbation (raids, winters). A steady-state machine is
  interesting for ten minutes.

## Multiplayer

No colony sim has done it, so there's no template. The [kernel](README.md)
fixes the base (lockstep, every machine holds the full state, pause and speed
as inputs, a headless peer, join as checkpoint plus fast-forward), and that
splits the question into two levels with different machinery.

**A session** is one map, one lockstep sim, two to six players, hours long.
Everyone simulates everything, by construction. So player count doesn't cap
the game, map size does: the slowest machine runs the whole map whether two
or six people are on it, the same cap as single player, shared. N colonies on
a map are N hot regions, the easy case for the kernel's parallelism, and a
colony in steady state is mostly asleep.

- One colony or several is the session's choice, because the mechanism is the
  same: the sim doesn't know who owns what. Ownership is which entities accept
  whose commands. Co-op on one colony hands each player a district or a crew
  (the tiers above). Several colonies on one map is the AoE2 shape, and where
  trade, raids and rivalry live.
- PvP inside a session is fine with the full state on every machine, as AoE2
  on a LAN was fine. The real problem is loss, not information: losing a
  ten-hour colony to a friend ends friendships. Mortal, short runs are the
  fix.
- An offline player's colony keeps running, driven by the automation they
  built. The abstraction tiers are also the offline AI, which is one more
  reason to make them good. If everyone leaves, the headless peer keeps
  ticking or the session pauses; sessions save and resume either way.

**A world** is where twenty colonies live, and Dwarf Fortress already found
the structure: one site at full resolution, the rest simulated abstractly,
travel as a transition. This is the one place to break "never abstract the
sim", which was a rule about control tiers inside one colony. A world is a
graph of sites. A site is a session while someone is in it and a summary when
nobody is. Interaction between sites is a transaction (a caravan, a raid
resolved as a timed event, a migrant), never a shared tick. That's what
Allgard and Apeiron spec for ships and stars, so the federated half of the
stack is the world layer and the lockstep half is the site layer. One server
or a federation is then a hosting choice.

Offline at the world level is the hard one, and no game has a good answer.
Clash of Clans and Rust let you raid absent players and everyone hates it;
Eve's vulnerability windows work but need a whole social layer. For now: an
absent player's site drops to abstract resolution and can't be entered at
full resolution, so what can happen to it is what the abstract sim allows,
and that sim is deliberately gentle to absent owners. That's a knob, not a
mechanism, and it stays open.

Twenty colonies in one tick never happens. That's the answer to performance.

## Numerics

Games step everything with explicit Euler every tick out of habit, and because
a fixed tick is trivially deterministic and lockstep. The kernel keeps the tick
as the clock. Inside it, far more can move to the event side than games
usually allow.

- **Discrete events.** State is a function of time between events. A walk is a
  closed-form trajectory plus an arrival event; hunger has a computable
  threshold-crossing tick. A needs-and-jobs colony is almost entirely
  event-driven if designed that way from the start, so the ASCII prototype
  should be. Events run inside ticks in the wake queue's fixed order.
- **Implicit methods for fields.** Explicit diffusion has a step-size limit
  and rings (Oxygen Not Included's gases). Backward Euler is stable at any
  step, so temperature can step rarely and large, and its damping makes
  things settle, which lets regions sleep. The cost is a sparse solve per
  region with a halo, with a fixed iteration count and reduction order for
  determinism. Dwarf Fortress's far-reaching water pressure is a Poisson
  solve per connected body, so this turns the doc's sequential case into the
  normal one.
- **Optimization for planning.** Job assignment is the assignment problem;
  greedy nearest-job is why dwarves cross the map for one sock. Hauling is a
  transportation problem, production ratios are a linear program, a district
  controller is min-cost flow. This gives the tiers a mechanism: the player
  sets an objective and constraints, the solver plans. The duals are prices,
  which is how the plan stays legible ("this haul cost 3, this job paid 5")
  and an economy display for free. The abstract off-screen site sim is the
  other natural home.
- **Not neural nets.** Only integer-quantised inference is deterministic, and
  it's opaque and unmoddable, the two things this genre can't afford. At most
  a surrogate for the off-screen world, and a fitted low-order model beats it
  there too.

## The game (a proposal)

The "why didn't we think of that" games are one new mechanic in a small
frame, never breadth. Into the Breach shows you exactly what the enemy does
next turn, and that one piece of perfect information made a tactics game into
pure decision-making. So the question is which mechanic this stack makes
possible that nobody else could build, and the smallest game it deserves.

**Forecasting.** The sim is deterministic and fast-forwards for free, so a
fork plus a run-ahead plus a ghost render gives the player the exact future
of the machine they built: run the colony a year ahead in a second, see the
famine in month eight, go back and change the plan. No sim game has this
because no sim is deterministic and fast enough to run ahead and come back,
which is what the kernel already aims at for joining.

**The boundary is the design.** What keeps it from being solved is what the
forecast can't see: other players' inputs, the world layer's events, and
whatever the game rolls from a sealed seed when it happens rather than at
world creation. Planning is done against the forecast. The crisis is the
unforecastable arriving, when the forecast turns out to have been the truth
about the colony and a lie about the world. In co-op the forecast is local
and free; across colonies it's an information game, since you can see what a
rival's colony does if they do nothing, and they know you can.

Everything else in these notes is the frame, and none of it is new alone:
mortal runs of a few hours in a persistent world so a forecast never has to
stretch past a season; agents as event-driven optimizers with prices as
explanations, which is what makes a forecast instant and lets the ghost say
why; tiers so the mid-game is a sequence of starts; lockstep co-op for two to
six; 90s crispness, because ghosts over a crisp tile world read instantly.
The numerics are load-bearing: a year in a second needs a colony that's
mostly asleep, closed-form movement and implicit fields.

Dropped: a polished multiplayer Dwarf Fortress (the megaproject, and "done
right" never surprised anyone); a govern-not-command game over rational
agents (Victoria at colony scale, and indirect control makes the start slow);
asymmetric co-op with a human storyteller on a conservation-law budget (new
and small, the best runner-up, but a mode rather than a spine, and it can be
added later without changing anything).

Scope: one biome, a couple hundred agents, a few hours per run. The sentence:
the colony sim where you can see the future, and the game is everything you
can't.

Build order: the sand benchmark, the ASCII needs-and-jobs prototype, then
forecast as a fork plus a fast-forward plus a diff printed in the terminal.
If a forecast of a famine in ASCII makes you change a plan, the game exists.

## Two prototypes, kept apart

The [kernel doc](README.md) names falling sand first, as a throughput benchmark:
cells only, and it exercises the colour passes and the reach rule. It says
nothing about whether a game on top is any good.

The other is needs and jobs in ASCII: one map, fifty agents, three resources,
one crisis, entities only, single-threaded, interpreter is fine, deterministic
with a replay log. If a famine tells a story in a terminal it will with sprites.
If it doesn't, no renderer saves it. Neither needs the other.

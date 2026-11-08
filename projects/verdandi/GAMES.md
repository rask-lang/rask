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

So tiers of abstraction are a design object from day one. Each tier is built
from the tier below, has a strictly smaller command surface than the sum of
its parts, and the sim resolution stays at the bottom. Name the unit of
command at each stage (a worker, a crew, a district, a settlement) and what
the player stops touching when the next unit appears.

## Mechanisms to build first

Four, and the rest is layers on top:

- Agents with needs and a scheduler that picks the next job from them. Cheap,
  and where every story in DF and RimWorld comes from.
- Spatial logistics. Things physically move and distance is cost, so layout is
  a decision instead of a menu.
- Resource loops with sinks, so an economy doesn't flatline.
- Scheduled perturbation (raids, winters). A steady-state machine is
  interesting for ten minutes.

## Two prototypes, kept apart

The [kernel doc](README.md) names falling sand first, as a throughput benchmark:
cells only, and it exercises the colour passes and the reach rule. It says
nothing about whether a game on top is any good.

The other is needs and jobs in ASCII: one map, fifty agents, three resources,
one crisis, entities only, single-threaded, interpreter is fine, deterministic
with a replay log. If a famine tells a story in a terminal it will with sprites.
If it doesn't, no renderer saves it. Neither needs the other.

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

Rule: a sim variable earns its place only if it can produce a consequence
the player can see or act on. The test is whether it can produce a story a
player would tell. DF's cats dying from licking spilled beer passes: a hidden
mechanism leaked into a visible mystery. DF's combat log fails: constantly
shown, no consequence beyond whether the unit dies. Two hundred body parts
pass only where they change what happens next (no hand, no shield; a scar
the giver remembers). Detail is pulled, not pushed. Legends mode is the right
model and the combat log the wrong one, in the same game.

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

## The game

**Forecasting, considered and dropped as the spine.** The sim is
deterministic and fast-forwards for free, so a fork plus a run-ahead could
show the player the exact future of their colony. In play that's a compile
loop: edit the plan, run a season ahead, see the famine, go back, edit. It
removes the one pleasure these notes start from, watching a machine you built
run and being surprised. It stays as a dev tool (a replay plus a diff) and
maybe as a late tier, a planner colonist whose skill sets how good a model of
the colony they run, with the abstract site sim as the model.

**DF 2.0 is the game**, and the fear of it is about the wrong thing. Dwarf
Fortress took twenty years through accretion, not structure. Its structure
is small: a world generated with history, one site at full resolution, the
rest abstract, a legends view. That's the sessions-and-worlds shape above,
which gets built anyway. RimWorld proved "DF but legible" alone is enough.
So this is DF's shape with less content on foundations that don't have its
four failures: it drowns at a hundred (tiers that compose), it can't be read
(the surfacing rule), it runs on one core (the numerics), it never had
multiplayer (lockstep co-op). No gimmick. The colony that runs itself the
way you taught it, and explains itself when it doesn't.

Two rules as the knife that keeps it finishable. The surfacing rule cuts
content: a variable that never shows on screen or in a decision isn't built,
which removes most of DF. Mortal runs cut time: a fortress lasts hours, so
the late game doesn't exist in version one. World layer, legends and
adventure mode are later versions, and the structure makes them additive.

DF is three games stuck together, and version one carries one as its heart
with the other two sketched. The digging game (a fortress carved into a
mountain). The people game (personalities, memories, the tantrum spiral).
The history game (a world that existed before you and remembers you after).
Digging is in, as the setting at least.

**Digging.** Finding a good hill and digging in is reading the terrain,
picking the entrance and carving rooms out of rock, and all of that is two
dimensional with the hill as the thing you're inside. Dungeon Keeper, the
most satisfying digging game made, is one flat level; Evil Genius is its
descendant. DF's hundred Z-levels multiply the complexity (3D pathing with
invalidation is its frame killer) and the slice view can't show the result,
so you pay for depth you can't see. A few discrete floors, each a crisp tile
map, the floor above cut away and the one below shaded (Going Medieval, Evil
Genius 2), keeps the fortress-in-a-mountain fantasy and the legibility.
Three to five floors is probably where the fantasy saturates. The kernel's
cube lattice covers it either way.

Why DF has no tall halls: not representation, DF has the levels. Nothing in
its sim cares about vertical volume (no light, air, span limit, sound or
prestige), so a three-level hall is three levels of digging for the room
value of one. A thing in a sim gets used only if a mechanism rewards or
punishes it. Real mining has four decisions games skip, and each passes the
story test: rock takes time, so a hall is a project; spoil has to go
somewhere, and if dug rock is the stone you build from, the fort is quarried
out of itself; spans have limits, so a wide hall needs pillars, which is what
makes volume matter; rock differs, so the hill has a geology you read before
you cut. Slow digging and the fast start reconcile through depth as the
ladder: soil in minutes, soft rock in a day, hard rock and ore and caverns
deeper.

Paths, not exclusive:

- **Dungeon Keeper.** A few floors, cheap digging, cubes. Known fun, nothing
  new. The fallback.
- **Volumes with physics.** Rooms are volumes. Light from a shaft diffuses
  through a hall, warm air rises, a forge needs a chimney, an unsupported
  span fails. Fields over cells with implicit solves, so the numerics are
  the game here.
- **Geology.** The hill is a seeded body: strata, a fault, a water table,
  veins, caverns. Reading the rock is the skill; prospecting, sounding and
  cores are the early tools. "Find a good hill" as a mechanic.
- **Mass conservation.** Every dug cell is material. Spoil is the quarry, the
  quarry is the wall. The federation docs' conservation idea, applied to
  rock.

Likely: the last three combined, the first as fallback. Geology gives the
start, conservation the loop, volumes the mid-game projects. The view then
borrows from architects: a plan at any height and a section at any vertical
plane, both crisp 2D drawings, which can show a tall hall where the slice
view never could. Evil Genius's cutaway is a section already.

Realism is a source, not a goal. Ventilation belongs only if smoke is
visible and suffocation is a consequence; a water table only if a flood is
a story.

## Setting

Low fantasy works for structural reasons. Zero-cost legibility: everyone
knows what a dwarf, a forge and a barrel are, the surfacing rule applied to
fiction. A reason to dig and a hand-scale economy where every item is made by
a person, which is the scale an agent sim works at. Visible technology, no
panel hiding the mechanism. Sci-fi fails on all three: a replicator is a
menu. So a setting has to supply those, plus perturbation and a reason for
the mining decisions and the fields, and whatever supplies them will feel
dwarf-shaped whatever it's called.

Magic is the replicator problem in another coat. As a spell list it's a menu
that bypasses the sim and breaks conservation. As a field in the rock that
diffuses, pools, depletes, has to be dug towards and channelled, it's one
more field over cells with an implicit solve, it obeys the same conservation
loop as stone, and it gives volume and geology a second reason to matter. No
colony sim has fields worth the name, so this is where a dark fantasy
setting could be the better fit. The one decision to make now: is there a
conserved field in the rock. Everything else about the setting waits until
after the ASCII prototype, which doesn't care what the agents are called.

The Norse source is one candidate given what the stack is named after:
dwarves are the underground smiths of the Eddas, the mountain is inhabited,
and the hill looks back at you.

The stronger candidate is an existing universe from another project, kept
out of these notes. Its fit comes from three things. Its cosmology is a
generation template (a flat disc with a central mountain and rings outward,
with up and centre as light and down and outward as the deep), its history
is a roll, and only its plot is one roll authored to the end, so the plot is
what's dropped, the same split DF makes. Its one law is a sim rule: energy
can be received but not taken, so it's conserved fields with a vertical
gradient, and extraction yields the energy plus a corrupt by-product that
scales with what was taken. The physics enforces the moral law; nothing is
narrated. And its dwarf-shaped people mine their own ancestors, so the
player's daily activity is the setting's temptation, deeper is richer and
deeper is worse, and the mortal run falls out of the theology instead of
being imposed. Its endgame, letting go instead of completing the grasp, is a
release mechanic at the top tier. A disc world also removes the sphere space
from the kernel's open list.

Its roster passes the test that matters: every faction is a different way of
bending the one conservation law, with a different by-product, so factions
are rule sets for the same engine rather than skins, which is what HoMM3's
factions are mechanically. Its ages of creation give generated history a
floor beneath it that wasn't generated, which is what "feels old" is.

Mystery in a sim is a hidden mechanism with visible consequences, the
surfacing rule again. A wiki can't be stopped, but it can be made useless:
keep the mechanism small and the interactions deep, and roll the field's
constants per world, so a wiki can state the law but never a recipe (Noita,
DF). In-game the law stays unnamed; the by-product is seen, never explained.

Risks: breadth (version one is one playable people, what climbs from below,
and the surface power as pressure at the world layer, never a second colony
on the map); legibility (original lore, familiar silhouette, one frame says
what a creature does); preaching (no meter with a sermon on it).

The question that decides whether the people game works with these people:
what does a stone-born Delver need? If the answer is to receive, then the
needs engine and the magic are one system, scarcity is a thin field, and
grasping is what a hungry agent does when receiving is too slow. Temptation
as hunger.

## Two prototypes, kept apart

The [kernel doc](README.md) names falling sand first, as a throughput benchmark:
cells only, and it exercises the colour passes and the reach rule. It says
nothing about whether a game on top is any good.

The other is needs and jobs in ASCII: one map, fifty agents, three resources,
one crisis, entities only, single-threaded, interpreter is fine, deterministic
with a replay log. If a famine tells a story in a terminal it will with sprites.
If it doesn't, no renderer saves it. Neither needs the other.

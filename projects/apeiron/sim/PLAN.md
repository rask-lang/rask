# Stage 0 Simulation Project

Throwaway Python. Validate game mechanics cheaply. Kill bad ideas before building anything in Rask.

## Core Idea

The common layer IS the simulation engine. A **world** contains **locations** and **agents**. Agents observe, decide, act. The world resolves actions and advances. Everything else — economy, crafting, exploration — is a configuration of this engine: which rules apply, which agent behaviors exist, what the locations contain.

The "economy sim" isn't a separate thing. It's the world sim with economic agents and trade rules plugged in.

## Structure

```
sim/
├── engine/
│   ├── __init__.py
│   ├── agent.py        # Agent: id, state, inventory, decide(obs) -> actions
│   ├── world.py        # World: locations, agents, rules, tick loop
│   ├── actions.py      # Action types: Move, Buy, Sell, Extract, Craft, ...
│   ├── rules.py        # Rule interface: validates + resolves actions
│   ├── observation.py  # Build per-agent observations, filtered by constraints
│   ├── constraints.py  # Tunable knobs: visibility, trade reach, info delay, rate limits
│   ├── record.py       # Append metrics per tick, dump CSV/JSON
│   ├── plot.py         # Matplotlib: time series, heatmap, histogram, scatter, 3D
│   └── sweep.py        # Run function across param combos with multiprocessing.Pool
├── rules/
│   ├── trade.py        # Order posting, matching, settlement
│   ├── extraction.py   # Mining from deposits
│   ├── crafting.py     # Interaction function, material transformation
│   ├── movement.py     # Moving between locations, fuel cost
│   └── minting.py      # Credit creation (activity-tied)
├── agents/
│   ├── extractor.py    # Mines deposits, sells raw materials
│   ├── hauler.py       # Buys low, flies, sells high
│   ├── station.py      # Market-maker: holds inventory, adjusts prices
│   ├── researcher.py   # Explores crafting space, discovers recipes
│   └── random.py       # Baseline: random valid actions
├── galaxy/
│   ├── generate.py     # Star placement algorithms (clustered + arms + bridges)
│   ├── resources.py    # Resource distribution per element table + abundance tiers
│   ├── topology.py     # Graph analysis: clusters, bridges, connectivity
│   └── run.py          # CLI: generate, analyze, visualize
├── crafting/
│   ├── interaction.py  # Core interact() function (peaks, shapes, energy)
│   ├── peaks.py        # Peak generation from seed
│   ├── strategies/     # Hill-climbing, grid search, random
│   └── run.py          # Terminal REPL
├── scenarios/
│   ├── founding_5.py   # 5 systems from galaxy gen, standard agents, standard rules
│   ├── sparse_3.py     # 3 isolated systems, fragile network test
│   ├── elements_5.py   # 5-element config for A/B comparison
│   └── elements_14.py  # 14-element config for A/B comparison
├── run_world.py        # CLI: run a scenario, record metrics, save plots
├── run_compare.py      # CLI: run two scenarios, compare outputs
└── transform_sim.py    # Old. Keep for reference.
```

## Engine

### agent.py

An agent is: an ID, a position (which location), inventory (dict), credits (float), and a `decide(observation) -> list[Action]` function. Agents are dataclasses + a function.

Different agent behaviors are different `decide` functions. Plug them in. The engine doesn't know what an "extractor" is — it just calls `decide` and resolves the returned actions.

### world.py

A world is: a list of locations, a list of agents, a list of active rules, a constraint config, a tick counter. Each tick:

1. Build observation per agent (filtered by constraints)
2. Each agent calls `decide(obs)`
3. Collect all actions
4. Each rule validates and resolves its action types
5. Update world state
6. Record metrics

The tick loop is a function: `tick(world) -> world`. No mutation, no side effects except recording. Testable. A "simulation run" is just `for _ in range(N): world = tick(world)`.

### actions.py

Tagged unions. The set of possible actions is open — rules define what actions they handle.

```python
Move(agent_id, to_location)
Buy(agent_id, item, qty, max_price)
Sell(agent_id, item, qty, min_price)
Extract(agent_id, deposit_id, amount)
Craft(agent_id, inputs, energy)
Mint(agent_id, amount, reason)
```

New action types = new rules. The engine doesn't hardcode what actions exist.

### rules.py

A rule is a function: `resolve(world, actions) -> world`. It takes the world and all submitted actions, validates them, resolves conflicts, and returns the updated world.

Rules are composable. The world sim runs with a list of rules: `[trade_rule, extraction_rule, movement_rule, minting_rule]`. Add crafting by adding `crafting_rule` to the list. Remove trade by removing `trade_rule`. Test movement in isolation by running only `movement_rule`.

Each rule only touches the action types it understands. Trade rule ignores Extract actions. Extraction rule ignores Buy/Sell. No coupling between rules.

### observation.py

Builds what an agent can see. Filtered by constraints:

- **Local**: agent sees its own location's state (market, resources, other agents)
- **Regional**: agent sees neighbors within N hops
- **Global**: agent sees everything

The observation is a plain dict. Agents never access the world directly — only through their observation. This enforces information constraints structurally.

### constraints.py

Tunable knobs:

| Knob | Values | Tests |
|------|--------|-------|
| `visibility` | `local` / `regional(N)` / `global` | Can the economy work without global price info? |
| `trade_reach` | `local` / `neighbors` / `global` | Must you trade locally or can you trade anywhere? |
| `info_delay` | `0..N` ticks | Does lagged info change behavior? |
| `rate_limit` | actions per tick per agent | Does bounding throughput matter? |

### record.py

`Recorder` object. Call `rec.add(tick, "total_credits", value)`. At end, `rec.to_csv("out.csv")` or `rec.to_json("out.json")`. Records are `list[tuple[int, str, float]]`. Flat, dumb, fast.

### plot.py

Thin matplotlib wrappers. Each function takes data + output path, saves a PNG.

- `time_series(data, labels, path)` — line chart, multiple series
- `heatmap(grid, xlabel, ylabel, path)` — 2D color grid
- `histogram(values, bins, path)` — distribution
- `scatter(xs, ys, path)` — 2D scatter
- `scatter_3d(xs, ys, zs, path)` — 3D scatter (for galaxy viz)

No interactive plots. Generate PNGs. Look at them.

### sweep.py

```python
results = sweep(
    func=run_scenario,
    params={
        "seed": range(10),
        "visibility": ["local", "global"],
        "num_systems": [3, 5, 10],
    },
    workers=8,
)
```

Cartesian product of params. Run each combo in a process pool. Return list of results.

## Rules

Each rule is a separate module. Swap, combine, modify independently.

### trade.py

Agents post Buy/Sell actions. Market matches by price-time priority within a location. Cross-location trade requires a hauler to physically move goods. Settlement is instant within a location.

### extraction.py

Agent submits Extract action on a deposit. If the deposit has remaining quantity and the agent is at the right location, deduct from deposit, add to agent inventory.

### movement.py

Agent submits Move action. Fuel cost = `f(distance, cargo_mass)`. If agent has enough fuel, move. Fuel is consumed (destroyed). The fuel cost function is a parameter — swap it.

### crafting.py

Agent submits Craft action with inputs and energy. Runs the interaction function (from `crafting/interaction.py`). Produces output materials. Consumes inputs. Mass loss applies.

### minting.py

Credit creation. Activity-tied: agents earn credits for completing actions (delivering cargo, extracting resources). The minting rate is a parameter.

## Agents

Each agent is a `decide(observation) -> list[Action]` function. Stateless between calls — all state lives in the agent's world-state entry (inventory, credits, position). The function can carry internal heuristic state via closure if needed.

**extractor.py** — Finds the best available deposit at current location. Extracts. Sells surplus on local market. Moves to a new location if local deposits are depleted.

**hauler.py** — Scans visible markets for price differences. Buys where cheap, moves, sells where expensive. Accounts for fuel cost. Greedy arbitrage.

**station.py** — Market maker. Holds inventory. Posts buy/sell orders with spread. Adjusts prices based on inventory levels: overstocked → lower ask, understocked → raise bid.

**researcher.py** — Runs crafting experiments. Tries compositions near known peaks. Records results. Can use strategies from `crafting/strategies/`.

**random.py** — Picks a random valid action each tick. Baseline for comparison.

## Galaxy

Generates the world's locations. Feeds directly into world sim scenarios.

**generate.py** — Star placement. Produces a list of locations with (x, y, z) positions. Algorithms: clustered cores, spiral arms, bridge stars, sparse frontier. Seeded, deterministic.

**resources.py** — Assigns element deposits to locations based on abundance tiers (common 90-100%, strategic 30-80%, exotic 5-15%). The element config (5 or 14 elements) plugs in here.

**topology.py** — Builds a graph from locations (edge if distance < jump_range). Computes: clusters, bridge stars (betweenness centrality), average connectivity, isolated nodes. Outputs analysis + data for plotting.

**run.py** — CLI entry. Generate galaxy, analyze, save plots. Also exports locations as data that scenarios can import.

A scenario like `founding_5.py` calls galaxy generation, picks 5 well-connected systems from a dense cluster, populates them with agents, and returns a configured World.

## Crafting

Standalone REPL for interactive experimentation. Also provides the interaction function that the `crafting` rule uses in the world sim.

**interaction.py** — `interact(elements, fractions, energy, seed) -> properties`. The core function. Evaluates peaks, interference, energy windows. Returns 6 property values.

**peaks.py** — Peak generation from seed. Shapes (gaussian, plateau, needle, ridge), per-property heights, energy windows, interference.

**run.py** — Terminal REPL:

```
> mix Fe=0.97 C=0.03 energy=30
  density: 42.3  hardness: 71.8  conductivity: 12.1
  reactivity: 3.2  stability: 88.4  radiance: 0.1

> auto hillclimb target=hardness budget=50
  Best: Fe=0.972 C=0.028 e=31.2  hardness=73.1

> landscape Fe C hardness
  [saves heatmap PNG]
```

## Scenarios

A scenario is a function: `build(seed) -> World`. It assembles locations (from galaxy gen or hand-placed), agents, rules, and constraints into a ready-to-run world.

**founding_5.py** — 5 systems from galaxy gen (dense cluster), extractors + haulers + stations, all rules active, local visibility. The primary economy test.

**sparse_3.py** — 3 isolated systems. Tests whether trade emerges across long distances.

**elements_5.py / elements_14.py** — Same topology and agents, different element configs. For A/B comparison.

New scenarios are cheap to write. Pick locations, pick agents, pick rules, pick constraints. Run.

## CLI Entry Points

**run_world.py** — Run a scenario for N ticks. Record metrics. Save plots.

```
python run_world.py --scenario founding_5 --seed 42 --ticks 10000 --out results/
```

**run_compare.py** — Run two scenarios with same seeds. Compare metrics. Save comparison plots.

```
python run_compare.py --a elements_5 --b elements_14 --seeds 10 --ticks 10000 --out compare/
```

**galaxy/run.py** — Generate and visualize galaxy.

```
python -m galaxy.run --seed 42 --stars 10000 --out galaxy/
```

**crafting/run.py** — Interactive REPL.

```
python -m crafting.run --seed 42
```

## Pass/Fail Criteria

### Economy (founding_5 scenario, 10K ticks)

- Credits don't hyperinflate (< 10x starting supply)
- At least 3 of 5 systems participate in trade (> 0 trade volume)
- No single agent accumulates > 50% of total supply
- Prices differ between systems (geographic scarcity creates spread)
- Fuel is consumed (movement is happening, sinks work)

### Crafting (REPL + automated strategies)

- Players can reason about results (nearby compositions give similar results)
- Peaks are discoverable by hill-climbing within 50-200 experiments
- The search space has gradient (not flat, not random spikes)

### Element Count (A/B comparison)

- 14 elements create more distinct trade routes than 5
- OR: 5 elements produce equivalent economic complexity (then simpler wins)

### Galaxy (10K stars)

- Visible structure: dense cores, sparse arms, bridge stars
- Bridge stars have measurably higher betweenness centrality
- Resource distribution creates geographic scarcity (no system has everything)

## Build Order

1. **engine/** — world, agent, actions, rules, observation, constraints, record, plot, sweep
2. **galaxy/** — generate, resources, topology, run. Proves plot.py. Produces locations.
3. **crafting/** — interaction, peaks, REPL. Standalone. Also provides crafting rule.
4. **rules/ + agents/ + scenarios/** — trade, extraction, movement, minting. Agent behaviors. Scenarios wiring it all together.
5. **run_world.py + run_compare.py** — CLI entry points. Run scenarios, compare, plot.

Steps 2 and 3 are independent and can be built in parallel. Step 4 is where the economy sim comes together — it's just a scenario using the engine with economic rules and agents.

## Dependencies

Python 3.10+. matplotlib. No other deps. Maybe numpy later if matrix math gets heavy, but start without it.

# Stage 0 Simulation Project

Throwaway Python. Validate game mechanics cheaply. Kill bad ideas before building anything in Rask.

## Structure

```
sim/
├── common/
│   ├── __init__.py
│   ├── agent.py        # Agent: id, state, inventory, decide(obs) -> actions
│   ├── world.py        # World: systems, agents, tick loop, observation filtering
│   ├── trade.py        # Order posting, matching, settlement
│   ├── constraints.py  # Tunable knobs: visibility, trade reach, info delay, rate limits
│   ├── record.py       # Append metrics per tick, dump CSV/JSON
│   ├── plot.py          # Matplotlib: time series, heatmap, histogram, scatter, 3D
│   └── sweep.py         # Run function across param combos with multiprocessing.Pool
├── economy/
│   ├── run.py           # CLI entry. Wire pieces, run, plot.
│   ├── system.py        # Star system: resources, deposits, facilities, local market
│   ├── agents/
│   │   ├── extractor.py # Mines deposits, sells raw materials
│   │   ├── hauler.py    # Buys low, flies, sells high
│   │   ├── station.py   # Market-maker: holds inventory, adjusts prices
│   │   └── random.py    # Baseline: random valid actions
│   ├── resources.py     # Finite deposits, extraction rates, element types
│   ├── fuel.py          # Fuel consumption model (distance * mass)
│   └── scenarios/
│       ├── founding_5.py    # 5 systems, standard element spread
│       └── sparse_3.py      # 3 isolated systems, test fragile networks
├── crafting/
│   ├── run.py           # Terminal REPL: type ratios, see results, history
│   ├── interaction.py   # Core interact() function (from transform_sim.py concepts)
│   ├── peaks.py         # Peak generation, shapes, energy windows
│   └── strategies/
│       ├── hillclimb.py # Automated hill-climbing
│       ├── grid.py      # Systematic grid search
│       └── random.py    # Random baseline
├── elements/
│   ├── run.py           # A/B harness: run economy+crafting with different element configs
│   └── configs/
│       ├── five.py      # 5 elements: Fe, C, H, Si, Cu
│       └── fourteen.py  # Full 14 element table
├── galaxy/
│   ├── run.py           # Generate stars, analyze topology, save plots
│   ├── placement.py     # Star position algorithms
│   ├── resources.py     # Resource distribution per element table + abundance tiers
│   └── topology.py      # Graph from distance threshold, cluster detection, bridges
└── transform_sim.py     # Old. Keep for reference.
```

## Common Layer

### agent.py

An agent is: an ID, a position (which system), inventory (dict), credits (float), and a `decide(observation) -> list[Action]` function. That's it. No base class hierarchy. Agents are dataclasses + a function.

Actions are tagged unions: `Move(to)`, `Buy(item, qty, price)`, `Sell(item, qty, price)`, `Extract(resource)`, `Craft(recipe)`. The world resolves them.

### world.py

Holds systems, agents, a tick counter. Each tick:
1. Build observations per agent (filtered by constraints)
2. Each agent decides
3. Resolve all actions (trades match, resources extract, ships move)
4. Record metrics

No inheritance. The tick loop is a function that takes a world and returns the next world. Functional, testable, swappable.

### constraints.py

Tunable knobs, not protocol simulation:

| Knob | Values | Tests |
|------|--------|-------|
| `visibility` | `local` / `regional` / `global` | Can the economy work without global price info? |
| `trade_reach` | `local` / `neighbors` / `global` | Must you trade locally or can you trade anywhere? |
| `info_delay` | `0..N` ticks | Does lagged info change behavior? |
| `rate_limit` | actions per tick per agent | Does bounding throughput matter? |

Run the same scenario with different constraint configs. Compare outcomes.

### trade.py

Agents post orders. Market matches by price-time priority within a system. Cross-system trade requires a hauler agent to physically move goods. Settlement is instant within a system.

No protocol details. No capabilities. No escrow. Just: can these two orders match? Yes → execute.

### record.py

`Recorder` object. Call `rec.add(tick, "total_credits", value)`. At end, `rec.to_csv("out.csv")` or `rec.to_json("out.json")`. Records are just `list[tuple[int, str, float]]`. Flat, dumb, fast.

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
    func=run_economy,
    params={
        "seed": range(10),
        "visibility": ["local", "global"],
        "num_systems": [3, 5, 10],
    },
    workers=8,
)
```

Cartesian product of params. Run each combo in a process pool. Return list of results. Each result includes the params + whatever the function returned (metrics dict).

## Economy Sim

### What it tests

Does a small economy (3-10 systems) produce functioning trade, stable-ish prices, and circulation? Or does it collapse, stagnate, or concentrate?

### Pieces (all swappable)

**system.py** — A system has: position (x,y), resource deposits (element -> remaining quantity), facilities (list), local market (list of orders). Systems are data. No methods that encode game logic.

**resources.py** — How deposits work. extract(deposit, amount) -> (extracted, remaining). Finite. Rate-limited per tick. Different element types with different abundances. This is where element configs plug in.

**fuel.py** — `fuel_cost(distance, cargo_mass) -> float`. A function. Swap it to test different fuel models.

**agents/** — Each agent type is a `decide(observation) -> actions` function. Extractors mine the best available deposit. Haulers find price differences and move goods. Stations adjust prices based on inventory levels. All simple heuristics — the point is testing the economy, not building smart AI.

**scenarios/** — A scenario is a function that returns a configured World. `founding_5()` returns 5 systems with standard resources, 2-3 agents per system, standard constraints. `sparse_3()` returns 3 far-apart systems to test fragile networks.

### Metrics to record

- Credits per agent per tick (wealth distribution)
- Total credits in circulation (inflation)
- Price per resource per system per tick (price convergence)
- Trade volume per tick (is anything happening?)
- Resource depletion per system (are sinks working?)
- Fuel consumed per tick (is movement happening?)

### Pass/fail (from ROADMAP.md)

- Credits don't hyperinflate
- At least 3 of 5 systems participate in trade
- No single agent accumulates >50% of total supply

## Crafting Sim

### What it tests

Is hill-climbing toward stoichiometric peaks fun? Can a player reason about results?

### The REPL

```
> mix Fe=0.97 C=0.03 energy=30
  density: 42.3  hardness: 71.8  conductivity: 12.1
  reactivity: 3.2  stability: 88.4  radiance: 0.1

> mix Fe=0.95 C=0.05 energy=30
  density: 41.1  hardness: 65.2  conductivity: 13.0
  reactivity: 4.1  stability: 82.1  radiance: 0.2

> history
  #1  Fe=0.97 C=0.03 e=30  hardness=71.8
  #2  Fe=0.95 C=0.05 e=30  hardness=65.2

> auto hillclimb target=hardness budget=50
  ... runs 50 experiments ...
  Best: Fe=0.972 C=0.028 e=31.2  hardness=73.1

> landscape Fe C hardness
  [saves heatmap PNG]
```

Commands: `mix`, `history`, `auto <strategy>`, `landscape`, `peaks`, `reset`, `seed <n>`.

### Pass/fail (from ROADMAP.md)

- Players can reason about results ("more carbon made it harder")
- Peaks are discoverable through experimentation, not random guessing
- The search space isn't too flat or too spiky

## Element Count Sim

### What it tests

Does 14 elements add depth over 5? Or just noise?

### Approach

Run economy sim twice: once with `configs/five.py`, once with `configs/fourteen.py`. Same seeds, same agent strategies, same constraints. Compare:

- Number of distinct trade routes (more elements = more routes?)
- System specialization (do systems export different things?)
- Recipe diversity (more elements = more useful recipes?)
- Price variance across systems (more scarcity = more variance?)

Also run crafting sim with both configs. Compare:
- Number of discoverable peaks
- Whether more elements make discovery harder or just broader

### Pass/fail (from ROADMAP.md)

- 14 elements create trade patterns that 5 don't
- OR: reducing to 5 loses nothing noticeable (then simpler is better)

## Galaxy Sim

### What it tests

Does the procedural galaxy have interesting topology? Clusters, bridges, chokepoints, frontiers?

### What it does

1. Generate 10K star positions (placement algorithm: TBD — clustered + arms + bridges)
2. Assign resources per element abundance tiers
3. Build connectivity graph (edge if distance < jump_range)
4. Analyze: cluster count, bridge stars (high betweenness centrality), isolated stars, average connectivity
5. Visualize: 2D/3D scatter colored by cluster, resource heatmaps, connectivity graph

### Pass/fail (from ROADMAP.md)

- Galaxy has visible structure — dense cores, sparse arms, bridge stars
- Structure creates meaningful gameplay differences
- Not uniform, not too regular

## Build Order

1. **common/** — record, plot, sweep, constraints, agent, world, trade
2. **galaxy/** — simplest sim, standalone, proves plot.py works
3. **crafting/** — REPL + interaction function, proves the mechanic
4. **economy/** — the big one, uses common/agent+world+trade
5. **elements/** — comparison harness, wraps economy + crafting

Each step is independently useful. Don't need step N+1 to get value from step N.

## Dependencies

Python 3.10+. matplotlib. No other deps. Maybe numpy if matrix math gets heavy, but start without it.

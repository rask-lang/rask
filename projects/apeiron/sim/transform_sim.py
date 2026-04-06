"""
Material transformation simulation for Apeiron.

Interaction function: stoichiometric peaks in composition-energy space.

Each element pair generates peaks at seed-determined ratios. Peaks have
varied shapes (broad/narrow/sharp-edged/flat-topped), energy windows,
and per-property strengths. Peaks can interfere (reinforce or cancel)
when they overlap. The desert between peaks has low-amplitude deterministic
noise with rare micro-spikes.

The algorithm is visible. The specific landscape is seed-determined.
"""

import math
import random
import sys

PROPERTY_NAMES = ["density", "hardness", "conductivity", "reactivity",
                  "stability", "radiance"]
THEORETICAL_MAX = [100.0] * 6
NUM_PROPS = 6


def _sf(key, count):
    """Deterministic key -> list of floats in [0,1)."""
    rng = random.Random(key)
    return [rng.random() for _ in range(count)]


# ---------------------------------------------------------------------------
# Peak shapes: varied, not always Gaussian
# ---------------------------------------------------------------------------

def _shape_gaussian(t):
    """Smooth bell. Forgiving — works over a range of compositions."""
    return math.exp(-3.0 * t * t)

def _shape_plateau(t):
    """Flat top, then drops off. Stable compound — works across a range,
    then cliff-edge failure."""
    if t < 0.4:
        return 1.0
    return max(0.0, 1.0 - ((t - 0.4) / 0.6) ** 2)

def _shape_needle(t):
    """Extremely narrow. Precise stoichiometry required."""
    return max(0.0, 1.0 - t * t * 20.0)

def _shape_ridge(t):
    """Broad but with a sharp central peak. Rewarding to find, more
    rewarding to optimize precisely."""
    broad = max(0.0, 1.0 - t * t * 2.0)
    sharp = math.exp(-30.0 * t * t)
    return 0.5 * broad + 0.5 * sharp

SHAPES = [_shape_gaussian, _shape_plateau, _shape_needle, _shape_ridge]


# ---------------------------------------------------------------------------
# Galaxy: generates and caches the full landscape from seed
# ---------------------------------------------------------------------------

class Galaxy:
    def __init__(self, seed, num_centers=30):
        self.seed = seed
        self.num_centers = num_centers
        self._peaks = {}      # (num_elements,) -> list of Peak
        self._elem_props = {} # element_id -> [base properties]
        self._pair_aff = {}   # (ei, ej) -> [per-property affinity]
        self._noise_cache = {}

    def _elem(self, ei):
        if ei not in self._elem_props:
            raw = _sf(f"{self.seed}:el:{ei}", NUM_PROPS)
            self._elem_props[ei] = [r * 3.0 for r in raw]
        return self._elem_props[ei]

    def _pair(self, ei, ej):
        key = (min(ei, ej), max(ei, ej))
        if key not in self._pair_aff:
            raw = _sf(f"{self.seed}:pair:{key[0]}:{key[1]}", NUM_PROPS)
            self._pair_aff[key] = [2.0 * r - 1.0 for r in raw]
        return self._pair_aff[key]

    def element_base(self, fractions):
        base = [0.0] * NUM_PROPS
        for i, f in enumerate(fractions):
            ep = self._elem(i)
            for p in range(NUM_PROPS):
                base[p] += f * ep[p]
        return base

    def _get_peaks(self, num_elements):
        """Generate stoichiometric peaks for all element pairs.
        Each pair gets 3-5 peaks at specific ratios."""
        if num_elements in self._peaks:
            return self._peaks[num_elements]

        peaks = []
        for ei in range(num_elements):
            for ej in range(ei + 1, num_elements):
                raw = _sf(f"{self.seed}:peaks:{ei}:{ej}", 200)
                n_peaks = 3 + int(raw[0] * 3)  # 3-5 peaks per pair
                idx = 1
                for pk in range(n_peaks):
                    # Stoichiometric ratio for this pair (where in the
                    # ei-ej axis the peak sits)
                    ratio = raw[idx]; idx += 1
                    # Peak height per property — some peaks boost hardness,
                    # others conductivity, etc.
                    heights = []
                    for p in range(NUM_PROPS):
                        # Pair affinity modulates: good pairs get tall peaks
                        pair_aff = self._pair(ei, ej)[p]
                        h = raw[idx] * (1.0 + pair_aff) * 8.0
                        heights.append(h)
                        idx += 1
                    # Width (how forgiving the stoichiometry is)
                    width = 0.03 + raw[idx] * 0.15; idx += 1
                    # Shape
                    shape_idx = int(raw[idx] * len(SHAPES)) % len(SHAPES); idx += 1
                    # Energy window: [lo, hi] — peak only active in this range
                    e_lo = raw[idx] * 0.7; idx += 1
                    e_hi = e_lo + 0.1 + raw[idx] * 0.4; idx += 1
                    # Interference sign
                    interference = 1.0 if raw[idx] > 0.3 else -1.0; idx += 1

                    # Catalyst sensitivity: which catalyst elements lower
                    # this peak's activation energy, and by how much.
                    # Each peak responds to 1-3 catalyst elements.
                    n_cat_sensitive = 1 + int(raw[idx] * 3); idx += 1
                    cat_effects = {}  # catalyst_element -> e_lo reduction
                    for _ in range(n_cat_sensitive):
                        cat_el = int(raw[idx] * 20); idx += 1
                        # Reduction: 30-80% of e_lo (catalyst makes it
                        # accessible at much lower energy)
                        reduction = 0.3 + raw[idx] * 0.5; idx += 1
                        cat_effects[cat_el] = reduction

                    # The best peaks tend to have HIGH energy thresholds.
                    # This is what makes catalysts valuable — the best
                    # materials are energy-gated, and catalysts lower the gate.
                    peak_quality = sum(heights) / len(heights)
                    if peak_quality > 6.0:
                        e_lo = max(e_lo, 0.35)  # good peaks need high energy

                    peaks.append({
                        "ei": ei, "ej": ej,
                        "ratio": ratio,
                        "heights": heights,
                        "width": width,
                        "shape": SHAPES[shape_idx],
                        "e_lo": e_lo, "e_hi": e_hi,
                        "interference": interference,
                        "cat_effects": cat_effects,
                    })

        self._peaks[num_elements] = peaks
        return peaks

    def get_beacon_peaks(self, num_elements, beacon_value):
        """Beacon-perturbed peaks. Same structure as base peaks but with
        shifted positions and modulated heights. The beacon value is a
        required input — without it, the landscape is undefined.

        Peaks stay near their base stoichiometric ratios (same general
        chemistry) but the exact optimum shifts each tick. Knowledge of
        the right neighborhood transfers across ticks; exact parameters
        don't.
        """
        cache_key = (num_elements, beacon_value)
        if hasattr(self, '_beacon_peaks') and cache_key in self._beacon_peaks:
            return self._beacon_peaks[cache_key]
        if not hasattr(self, '_beacon_peaks'):
            self._beacon_peaks = {}

        base_peaks = self._get_peaks(num_elements)

        # Beacon-specific perturbations — deterministic from beacon value
        bp_raw = _sf(f"{self.seed}:beacon:{beacon_value}", len(base_peaks) * 4)

        perturbed = []
        for i, pk in enumerate(base_peaks):
            ri = i * 4
            # Shift ratio: ±100% of peak width (exact position unpredictable,
            # but still in the same neighborhood)
            ratio_shift = (bp_raw[ri] - 0.5) * pk["width"] * 2.0
            # Height modulation: 30%-170% (which peak is strongest changes)
            height_mod = 0.3 + bp_raw[ri + 1] * 1.4
            # Width modulation: 60%-140% (peak sharpness varies)
            width_mod = 0.6 + bp_raw[ri + 2] * 0.8
            # Energy window shift: ±10% of range
            e_shift = (bp_raw[ri + 3] - 0.5) * 0.2

            perturbed.append({
                "ei": pk["ei"], "ej": pk["ej"],
                "ratio": pk["ratio"] + ratio_shift,
                "heights": [h * height_mod for h in pk["heights"]],
                "width": pk["width"] * width_mod,
                "shape": pk["shape"],
                "e_lo": max(0, pk["e_lo"] + e_shift),
                "e_hi": min(1, pk["e_hi"] + e_shift),
                "interference": pk["interference"],
                "cat_effects": pk["cat_effects"],
            })

        self._beacon_peaks[cache_key] = perturbed
        return perturbed



def _desert_noise(point, seed, prop_idx):
    """Deterministic low-amplitude noise in the desert. Occasionally
    has a micro-spike — enough to notice, not enough to be systematic."""
    # Mix coordinates into a single hash input
    v = 0.0
    for i, p in enumerate(point):
        v += p * (i * 7.3 + 1.1)
    v += seed * 0.0001 + prop_idx * 3.7
    # Multi-frequency noise
    n = math.sin(v * 17.3) * 0.3
    n += math.sin(v * 47.1) * 0.15
    n += math.sin(v * 131.7) * 0.05
    # Rare micro-spike: sin^8 creates narrow spikes from smooth function
    spike = math.sin(v * 7.7 + prop_idx) ** 8 * 2.0
    return n + spike


def interact(fracs, energy, galaxy, catalyst=None, precision=None,
             beacon=None):
    """Core interaction function.

    fracs: element fractions summing to ~1.0
    energy: energy per mass
    galaxy: Galaxy instance
    catalyst: optional (element_index, fraction)
    precision: optional scatter magnitude (facility quality)
    beacon: optional int — beacon tick value. When provided, the function
            evaluates against beacon-perturbed peaks. Without it, uses
            base peaks (simulation/offline mode only — in-game, beacon
            is always required).
    """
    fracs = list(fracs)
    ne = len(fracs)

    # Precision noise (facility quality — separate from beacon)
    if precision and precision > 0:
        if beacon is not None:
            # Deterministic from beacon — verifiable
            noise_rng = random.Random(f"precision:{beacon}:{id(fracs)}")
            fracs = [max(0.001, f + noise_rng.gauss(0, precision)) for f in fracs]
        else:
            fracs = [max(0.001, f + random.gauss(0, precision)) for f in fracs]
        t = sum(fracs)
        fracs = [f / t for f in fracs]

    max_energy = 100.0
    energy_norm = min(energy / max_energy, 1.0)

    # Build input point (independent fracs + energy)
    point = list(fracs[:-1]) + [energy_norm]

    # Base properties: weighted average (boring, predictable)
    base = galaxy.element_base(fracs)

    active_catalyst = catalyst[0] if catalyst else None

    # Sum peak contributions — beacon-perturbed if beacon provided
    if beacon is not None:
        peaks = galaxy.get_beacon_peaks(ne, beacon)
    else:
        peaks = galaxy._get_peaks(ne)
    modification = [0.0] * NUM_PROPS

    for pk in peaks:
        ei, ej = pk["ei"], pk["ej"]
        ratio = pk["ratio"]

        # How close is the current composition to this peak's stoichiometry?
        fi, fj = fracs[ei], fracs[ej]
        pair_total = fi + fj
        if pair_total < 0.01:
            continue
        actual_ratio = fi / pair_total
        composition_dist = abs(actual_ratio - ratio) / pk["width"]

        # How much of the mix is this pair? Peaks are stronger when the
        # pair dominates the composition
        pair_weight = pair_total

        # Energy window — catalyst lowers the activation threshold
        effective_e_lo = pk["e_lo"]
        if active_catalyst is not None and active_catalyst in pk["cat_effects"]:
            reduction = pk["cat_effects"][active_catalyst]
            effective_e_lo *= (1.0 - reduction)  # e.g., 0.5 threshold * 0.4 = 0.2

        if energy_norm < effective_e_lo or energy_norm > pk["e_hi"]:
            continue  # outside energy window — peak inactive

        # Energy position within window (peaks are strongest at center)
        e_center = (effective_e_lo + pk["e_hi"]) / 2
        e_half = (pk["e_hi"] - pk["e_lo"]) / 2
        e_dist = abs(energy_norm - e_center) / e_half
        e_factor = max(0.0, 1.0 - e_dist * e_dist)

        # Shape determines falloff from stoichiometric ratio
        shape_val = pk["shape"](composition_dist)

        # Combine
        for p in range(NUM_PROPS):
            contrib = (pk["heights"][p] * shape_val * pair_weight
                       * e_factor * pk["interference"])
            modification[p] += contrib

    # Add desert noise where peaks are weak
    for p in range(NUM_PROPS):
        if abs(modification[p]) < 1.0:
            noise = _desert_noise(point, galaxy.seed, p)
            modification[p] += noise

    # Saturation curve
    result = []
    for p in range(NUM_PROPS):
        raw = base[p] + modification[p]
        t_max = THEORETICAL_MAX[p]
        if raw > 0:
            result.append(t_max * (1.0 - math.exp(-raw / t_max)))
        else:
            result.append(-t_max * (1.0 - math.exp(raw / t_max)))
    return result


# ---------------------------------------------------------------------------
# Researcher agents
# ---------------------------------------------------------------------------

class Researcher:
    def __init__(self, galaxy, num_elements, budget, strategy, max_energy=50,
                 catalyst=None, precision=None):
        self.galaxy = galaxy
        self.ne = num_elements
        self.budget = budget
        self.strategy = strategy
        self.max_energy = max_energy
        self.catalyst = catalyst
        self.precision = precision
        self.target_prop = 1
        self.exp_count = 0
        self.best_val = -float('inf')
        self.best_point = None
        self.best_props = None
        self.improvements = 0
        self.history = []
        self.all_props = []
        self.all_points = []
        self.all_values = []
        self.current = self._rand()

    def _rand(self):
        raw = [random.random() for _ in range(self.ne)]
        t = sum(raw)
        return [r / t for r in raw], random.uniform(0, self.max_energy)

    def _perturb(self, pt, scale=0.03):
        f, e = pt
        nf = [max(0.001, fi + random.gauss(0, scale)) for fi in f]
        t = sum(nf)
        nf = [fi / t for fi in nf]
        ne = max(0, min(self.max_energy, e + random.gauss(0, scale * 100)))
        return nf, ne

    def _eval(self, pt):
        f, e = pt
        props = interact(f, e, self.galaxy, self.catalyst, self.precision)
        v = props[self.target_prop]
        self.exp_count += 1
        self.budget -= 1
        if v > self.best_val:
            self.best_val = v
            self.best_point = pt
            self.best_props = props
            self.improvements += 1
        self.history.append((self.exp_count, self.best_val))
        self.all_props.append(props)
        self.all_points.append(pt)
        self.all_values.append(v)
        return props, v

    def absorb(self, other):
        """Absorb another researcher's knowledge."""
        evals = sorted(zip(other.all_points, other.all_values),
                       key=lambda x: x[1], reverse=True)
        self._leads = [pt for pt, _ in evals[:10]]
        self.current = other.best_point
        self.best_val = other.best_val
        self.best_props = other.best_props
        self.best_point = other.best_point

    def _smart_restart(self):
        if hasattr(self, '_leads') and self._leads and random.random() < 0.7:
            pt = random.choice(self._leads)
            return self._perturb(pt, scale=0.15)
        return self._rand()

    def run(self):
        stall = 0
        while self.budget >= 1:
            if self.strategy == "random":
                self._eval(self._rand())
            elif self.strategy == "hillclimb":
                pt = self._perturb(self.current)
                _, v = self._eval(pt)
                if v >= self.best_val:
                    self.current = pt
            elif self.strategy == "smart":
                pt = self._perturb(self.current)
                old = self.best_val
                _, v = self._eval(pt)
                if v >= old:
                    self.current = pt
                    stall = 0
                else:
                    stall += 1
                if stall > 30:
                    self.current = self._smart_restart()
                    stall = 0
        return self.history


# ---------------------------------------------------------------------------
# Metrics + runner
# ---------------------------------------------------------------------------

def _val_at(hist, cp):
    best = 0
    for n, v in hist:
        if n <= cp:
            best = v
    return best


def boundary_crossings(all_props, threshold=2.0):
    c = 0
    for i in range(1, len(all_props)):
        d = sum((a - b)**2 for a, b in zip(all_props[i-1], all_props[i])) ** 0.5
        if d > threshold:
            c += 1
    return c


def run_sim(galaxy_seed=42, ne=3, budget=500, nr=20, tp=1, nc=30,
            max_e=50, catalyst=None, precision=None):
    g = Galaxy(galaxy_seed, nc)
    data = {}
    for s in ["random", "hillclimb", "smart"]:
        runs = []
        for run in range(nr):
            random.seed(run * 1000 + hash(s))
            r = Researcher(g, ne, budget, s, max_e, catalyst, precision)
            r.target_prop = tp
            r.run()
            runs.append(r)
        data[s] = runs
    return data


def print_curves(data):
    cps = [10, 50, 100, 200, 500]
    print(f"\n{'Strategy':<12} ", end="")
    for cp in cps:
        print(f"{'@'+str(cp):>8}", end="")
    print(f"  {'final':>8}")
    print("-" * 70)
    for s, runs in data.items():
        print(f"{s:<12} ", end="")
        for cp in cps:
            vals = [_val_at(r.history, cp) for r in runs]
            print(f"{sum(vals)/len(vals):>8.3f}", end="")
        finals = [r.history[-1][1] for r in runs]
        print(f"  {sum(finals)/len(finals):>8.3f}")


def print_metrics(data):
    print(f"\n{'Strategy':<12} {'crossings':>10} {'efficiency':>11}")
    print("-" * 40)
    for s, runs in data.items():
        cx = [boundary_crossings(r.all_props) for r in runs]
        ef = [r.improvements / max(r.exp_count, 1) for r in runs]
        print(f"{s:<12} {sum(cx)/len(cx):>10.1f} {sum(ef)/len(ef):>10.1%}")


# ---------------------------------------------------------------------------
# Tests
# ---------------------------------------------------------------------------

def test_basic():
    print("=" * 70)
    print("BASIC: Strategy comparison (3 elements, budget 500)")
    print("=" * 70)
    data = run_sim()
    print_curves(data)
    print_metrics(data)


def test_catalyst():
    print("\n" + "=" * 70)
    print("CATALYST: Do catalysts lower activation energy?")
    print("=" * 70)
    print("\n  Catalysts don't change what's thermodynamically possible —")
    print("  they change what's kinetically accessible at a given energy level.")
    print("  Test: at LOW energy (max=20), which catalysts unlock better peaks?\n")

    # Low energy: many good peaks are energy-gated above e=0.35
    d0 = run_sim(budget=500, max_e=20)
    base = sum(r.best_val for r in d0["smart"]) / 20
    print(f"  No catalyst (max_e=20): smart avg = {base:.3f}")

    for cat in range(15):
        d = run_sim(budget=500, max_e=20, catalyst=(cat, 0.1))
        avg = sum(r.best_val for r in d["smart"]) / 20
        pct = (avg - base) / abs(base) * 100 if abs(base) > 0.01 else 0
        bar = "#" * max(0, int(pct / 3)) if pct > 0 else ""
        print(f"  Catalyst {cat:>2}: {avg:.3f} ({pct:+5.1f}%) {bar}")


def test_energy():
    print("\n" + "=" * 70)
    print("ENERGY GATING: Do higher energy levels access better regions?")
    print("=" * 70)
    for me in [5, 15, 30, 50, 80]:
        d = run_sim(max_e=me)
        sf = sum(r.history[-1][1] for r in d["smart"]) / 20
        rf = sum(r.history[-1][1] for r in d["random"]) / 20
        print(f"  Max energy {me:>3}: smart={sf:.3f}  random={rf:.3f}")


def test_precision():
    print("\n" + "=" * 70)
    print("PRECISION: Manufacturing yield at different facility qualities")
    print("=" * 70)

    g = Galaxy(42)
    nr_batches = 100  # manufacturing runs per precision level

    # Phase 1: Discovery — find the best material with a perfect lab
    random.seed(42)
    discoverer = Researcher(g, 3, 1000, "smart")
    discoverer.run()
    recipe = discoverer.best_point  # the exact composition + energy
    target_val = discoverer.best_val
    target_props = discoverer.best_props
    fracs, energy = recipe
    print(f"\n  Discovered recipe: hardness = {target_val:.3f}")
    print(f"  Composition: {[f'{f:.3f}' for f in fracs]}, energy: {energy:.1f}")

    # Phase 2: Manufacturing — reproduce the recipe at different precisions
    # Each "batch" evaluates the recipe once with precision noise
    print(f"\n  {'Precision':>10} {'mean':>8} {'std':>8} {'yield>90%':>10} {'yield>80%':>10}")
    print("  " + "-" * 50)

    for prec in [0, 0.001, 0.005, 0.01, 0.05, 0.1, 0.2]:
        values = []
        for batch in range(nr_batches):
            random.seed(batch * 777)
            props = interact(fracs, energy, g, precision=prec if prec > 0 else None)
            values.append(props[1])  # hardness

        mean = sum(values) / len(values)
        std = (sum((v - mean)**2 for v in values) / len(values)) ** 0.5
        yield_90 = sum(1 for v in values if v > target_val * 0.9) / len(values)
        yield_80 = sum(1 for v in values if v > target_val * 0.8) / len(values)
        label = "perfect" if prec == 0 else f"±{prec}"
        print(f"  {label:>10} {mean:>8.3f} {std:>8.3f} {yield_90:>10.0%} {yield_80:>10.0%}")


def test_sharing():
    print("\n" + "=" * 70)
    print("KNOWLEDGE SHARING: How much does sharing accelerate?")
    print("=" * 70)
    g = Galaxy(42)
    nr = 20

    for ne in [3, 4, 5]:
        print(f"\n--- {ne} elements ---")
        indep = []
        for run in range(nr):
            random.seed(run * 1000)
            r1 = Researcher(g, ne, 250, "smart")
            r1.run()
            random.seed(run * 1000 + 500)
            r2 = Researcher(g, ne, 250, "smart")
            r2.run()
            indep.append(max(r1.best_val, r2.best_val))

        shared = []
        for run in range(nr):
            random.seed(run * 1000)
            r1 = Researcher(g, ne, 250, "smart")
            r1.run()
            random.seed(run * 1000 + 500)
            r2 = Researcher(g, ne, 250, "smart")
            r2.absorb(r1)
            r2.run()
            shared.append(max(r1.best_val, r2.best_val))

        im = sum(indep) / nr
        shm = sum(shared) / nr
        print(f"  Independent: {im:.3f}")
        print(f"  Shared:      {shm:.3f}  ({shm/im:.2f}x)")


def test_reverse():
    print("\n" + "=" * 70)
    print("REVERSE ENGINEERING: experiments to reproduce known result?")
    print("=" * 70)
    g = Galaxy(42)
    random.seed(42)
    disc = Researcher(g, 3, 500, "smart")
    disc.run()
    target = disc.best_props
    print(f"  Target hardness: {disc.best_val:.3f}")
    for tol in [0.5, 1.0, 2.0, 5.0]:
        found = []
        for run in range(20):
            random.seed(run * 2000)
            hit = None
            for exp in range(2000):
                raw = [random.random() for _ in range(3)]
                t = sum(raw)
                f = [r/t for r in raw]
                e = random.uniform(0, 50)
                props = interact(f, e, g)
                d = sum((a-b)**2 for a, b in zip(props, target)) ** 0.5
                if d < tol:
                    hit = exp + 1
                    break
            found.append(hit)
        ok = [f for f in found if f is not None]
        if ok:
            print(f"  Tol {tol:.1f}: {len(ok)}/20 found, avg {sum(ok)/len(ok):.0f} exp")
        else:
            print(f"  Tol {tol:.1f}: 0/20 found in 2000 exp")


def test_seeds():
    print("\n" + "=" * 70)
    print("SEED VARIANCE: Different galaxies, different landscapes?")
    print("=" * 70)
    print(f"{'Seed':>6} {'random':>10} {'smart':>10}")
    print("-" * 30)
    vals = {"random": [], "smart": []}
    for seed in range(20):
        d = run_sim(galaxy_seed=seed, nr=10, budget=300)
        for s in ["random", "smart"]:
            f = sum(r.history[-1][1] for r in d[s]) / 10
            vals[s].append(f)
        print(f"{seed:>6} {vals['random'][-1]:>10.3f} {vals['smart'][-1]:>10.3f}")
    print("-" * 30)
    for label, fn in [("mean", lambda v: sum(v)/len(v)),
                      ("std", lambda v: (sum((x-sum(v)/len(v))**2 for x in v)/len(v))**0.5),
                      ("min", min), ("max", max)]:
        print(f"{label:>6}", end="")
        for s in ["random", "smart"]:
            print(f"{fn(vals[s]):>10.3f}", end="")
        print()


def test_elements():
    print("\n" + "=" * 70)
    print("ELEMENTS: Search difficulty vs element count")
    print("=" * 70)
    for n in [2, 3, 4, 5]:
        print(f"\n--- {n} elements ---")
        d = run_sim(ne=n, budget=1000)
        print_curves(d)


def test_density():
    print("\n" + "=" * 70)
    print("PHASE DENSITY: Regions per unit space")
    print("=" * 70)
    for nc in [10, 20, 30, 50, 100]:
        print(f"\n--- {nc} centers ---")
        d = run_sim(nc=nc)
        print_curves(d)


# ---------------------------------------------------------------------------
# Visualization
# ---------------------------------------------------------------------------

CHARS = " .:-=+*#%@"

def _ascii_heatmap(grid, rows=40, cols=80):
    """Render a 2D grid as ASCII. Returns list of strings + (lo, hi)."""
    lo = min(min(r) for r in grid)
    hi = max(max(r) for r in grid)
    res_y, res_x = len(grid), len(grid[0])
    lines = []
    for ey in range(0, res_y, max(1, res_y // rows)):
        line = ""
        for rx in range(0, res_x, max(1, res_x // cols)):
            v = grid[ey][rx]
            i = int((v - lo) / (hi - lo + 1e-9) * (len(CHARS) - 1))
            line += CHARS[i]
        lines.append(line)
    return lines, lo, hi


def _sample_grid_2elem(galaxy, prop, res=200, max_energy=50.0,
                       catalyst=None):
    """Sample property over ratio × energy for 2-element system."""
    grid = []
    for ey in range(res):
        row = []
        energy = ey / res * max_energy
        for rx in range(res):
            ratio = rx / res
            props = interact([ratio, 1.0 - ratio], energy, galaxy,
                             catalyst=catalyst)
            row.append(props[prop])
        grid.append(row)
    return grid


def viz_landscape(seed, prop=1):
    """Single property heatmap for 2-element system."""
    g = Galaxy(seed)
    grid = _sample_grid_2elem(g, prop)
    lines, lo, hi = _ascii_heatmap(grid)
    print(f"\n{PROPERTY_NAMES[prop]} (seed={seed})")
    print(f"X: element ratio [0,1]  Y: energy [0,50]  range [{lo:.2f}, {hi:.2f}]")
    for line in lines:
        print(line)


def viz_all_properties(seed):
    """Side-by-side heatmaps for all 6 properties."""
    g = Galaxy(seed)
    grids = [_sample_grid_2elem(g, p) for p in range(NUM_PROPS)]
    all_lines = []
    for p in range(NUM_PROPS):
        lines, lo, hi = _ascii_heatmap(grids[p], rows=20, cols=30)
        all_lines.append((PROPERTY_NAMES[p], lines, lo, hi))

    # Print in pairs
    for i in range(0, NUM_PROPS, 2):
        a = all_lines[i]
        b = all_lines[i + 1] if i + 1 < NUM_PROPS else None
        header = f"  {a[0]:^30s} [{a[2]:.1f},{a[3]:.1f}]"
        if b:
            header += f"    {b[0]:^30s} [{b[2]:.1f},{b[3]:.1f}]"
        print(header)
        for row in range(len(a[1])):
            line = "  " + a[1][row]
            if b and row < len(b[1]):
                line += "    " + b[1][row]
            print(line)
        print()


def viz_catalyst_compare(seed, cat_elem=5, cat_frac=0.1, prop=1):
    """Side-by-side: without vs with catalyst."""
    g = Galaxy(seed)
    grid_no = _sample_grid_2elem(g, prop)
    grid_yes = _sample_grid_2elem(g, prop, catalyst=(cat_elem, cat_frac))
    lines_no, lo_no, hi_no = _ascii_heatmap(grid_no, rows=30, cols=35)
    lines_yes, lo_yes, hi_yes = _ascii_heatmap(grid_yes, rows=30, cols=35)

    print(f"\n{PROPERTY_NAMES[prop]}: no catalyst vs catalyst {cat_elem} @ {cat_frac}")
    print(f"  {'No catalyst':^35s} [{lo_no:.1f},{hi_no:.1f}]"
          f"    {'With catalyst':^35s} [{lo_yes:.1f},{hi_yes:.1f}]")
    for i in range(max(len(lines_no), len(lines_yes))):
        a = lines_no[i] if i < len(lines_no) else ""
        b = lines_yes[i] if i < len(lines_yes) else ""
        print(f"  {a:<35s}    {b}")


def viz_energy_slices(seed, prop=1):
    """Landscape at fixed energy levels — shows energy gating."""
    g = Galaxy(seed)
    energies = [5, 15, 30, 50, 80]
    res = 200

    print(f"\n{PROPERTY_NAMES[prop]} at fixed energy levels (seed={seed})")
    for energy in energies:
        values = []
        for rx in range(res):
            ratio = rx / res
            props = interact([ratio, 1.0 - ratio], energy, g)
            values.append(props[prop])

        lo, hi = min(values), max(values)
        # Compress to ~60 chars
        line = ""
        for rx in range(0, res, max(1, res // 60)):
            v = values[rx]
            i = int((v - lo) / (hi - lo + 1e-9) * (len(CHARS) - 1))
            line += CHARS[i]
        print(f"  E={energy:>3}: {line}  [{lo:.1f},{hi:.1f}]")


def viz_ternary(seed, energy=30.0, prop=1):
    """Ternary phase diagram for 3-element system at fixed energy."""
    g = Galaxy(seed)
    res = 60
    grid = {}
    lo, hi = float('inf'), float('-inf')

    # Sample on a triangular grid (f0 + f1 + f2 = 1)
    for row in range(res + 1):
        f2 = row / res
        for col in range(res - row + 1):
            f0 = col / res
            f1 = 1.0 - f0 - f2
            if f1 < -0.001:
                continue
            f1 = max(0, f1)
            props = interact([f0, f1, f2], energy, g)
            v = props[prop]
            grid[(row, col)] = v
            lo = min(lo, v)
            hi = max(hi, v)

    print(f"\nTernary: {PROPERTY_NAMES[prop]} (seed={seed}, energy={energy})")
    print(f"Range [{lo:.2f}, {hi:.2f}]")
    print(f"Bottom-left: elem 0, bottom-right: elem 1, top: elem 2")
    for row in range(res, -1, -2):
        indent = " " * ((res - row) // 2)
        line = indent
        for col in range(res - row + 1):
            if (row, col) in grid:
                v = grid[(row, col)]
                i = int((v - lo) / (hi - lo + 1e-9) * (len(CHARS) - 1))
                line += CHARS[i]
            else:
                line += " "
        print(line)


def viz_trajectory(seed, strategy="smart", budget=200, prop=1):
    """Show a researcher's path through the 2-element landscape."""
    g = Galaxy(seed)
    random.seed(seed)
    r = Researcher(g, 2, budget, strategy)
    r.target_prop = prop
    r.run()

    # Sample landscape
    res = 200
    grid = _sample_grid_2elem(g, prop, res=res)
    lines, lo, hi = _ascii_heatmap(grid, rows=40, cols=80)

    # Convert to mutable char arrays
    display = [list(line) for line in lines]

    # Overlay trajectory
    for pt, val in zip(r.all_points, r.all_values):
        fracs, energy = pt
        rx = int(fracs[0] * 80)
        ey = int(energy / 50.0 * 40)
        rx = max(0, min(79, rx))
        ey = max(0, min(len(display) - 1, ey))
        if val >= r.best_val * 0.9:
            display[ey][rx] = '!'  # near-best experiments
        elif val > lo + (hi - lo) * 0.3:
            display[ey][rx] = 'o'  # decent finds
        else:
            display[ey][rx] = '·'  # desert exploration

    print(f"\n{strategy} trajectory: {PROPERTY_NAMES[prop]} (seed={seed}, budget={budget})")
    print(f"· = explored  o = decent find  ! = near-best")
    print(f"Best: {r.best_val:.3f} at experiment {r.improvements}")
    for line in display:
        print("".join(line))


def viz_property_scatter(seed, p1=1, p2=2, n_samples=500):
    """Scatter plot of two properties — shows correlation/tradeoffs."""
    g = Galaxy(seed)
    pts_x, pts_y = [], []
    for _ in range(n_samples):
        f = [random.random() for _ in range(3)]
        t = sum(f)
        f = [x / t for x in f]
        e = random.uniform(0, 50)
        props = interact(f, e, g)
        pts_x.append(props[p1])
        pts_y.append(props[p2])

    # ASCII scatter into a grid
    rows, cols = 25, 60
    x_lo, x_hi = min(pts_x), max(pts_x)
    y_lo, y_hi = min(pts_y), max(pts_y)
    canvas = [[' '] * cols for _ in range(rows)]
    counts = [[0] * cols for _ in range(rows)]

    for x, y in zip(pts_x, pts_y):
        c = int((x - x_lo) / (x_hi - x_lo + 1e-9) * (cols - 1))
        r = rows - 1 - int((y - y_lo) / (y_hi - y_lo + 1e-9) * (rows - 1))
        c = max(0, min(cols - 1, c))
        r = max(0, min(rows - 1, r))
        counts[r][c] += 1

    density_chars = " ·:+*#"
    for r in range(rows):
        for c in range(cols):
            n = counts[r][c]
            if n == 0:
                canvas[r][c] = ' '
            else:
                i = min(len(density_chars) - 1, n)
                canvas[r][c] = density_chars[i]

    print(f"\n{PROPERTY_NAMES[p1]} vs {PROPERTY_NAMES[p2]} ({n_samples} random samples, seed={seed})")
    print(f"X: {PROPERTY_NAMES[p1]} [{x_lo:.1f}, {x_hi:.1f}]")
    print(f"Y: {PROPERTY_NAMES[p2]} [{y_lo:.1f}, {y_hi:.1f}]")
    for row in canvas:
        print("  |" + "".join(row) + "|")
    print("  +" + "-" * cols + "+")


# ---------------------------------------------------------------------------
# Beacon tests
# ---------------------------------------------------------------------------

def test_beacon_batches():
    """Blind batches within tick windows.

    Each tick window: researcher commits N experiments (batch), then beacon
    reveals. All N experiments in the batch use the same beacon value.
    Between windows, the researcher can adapt (hill-climb across windows).

    Question: how does batch size affect discovery? Large batches = more
    coverage per tick but no learning within the batch.
    """
    print("\n" + "=" * 70)
    print("BEACON BATCHES: Batch size vs discovery rate")
    print("=" * 70)
    print("\n  Each tick: commit batch of N experiments, beacon reveals,")
    print("  evaluate all N with beacon scatter, learn, repeat.")
    print("  Total budget = 500 experiments across all ticks.\n")

    g = Galaxy(42)
    nr = 20
    total_budget = 500
    facility_precision = 0.05  # moderate facility

    for batch_size in [1, 5, 10, 25, 50, 100]:
        n_ticks = total_budget // batch_size
        finals = []

        for run in range(nr):
            random.seed(run * 1000)
            best_val = -float('inf')
            best_point = None
            current = None  # hill-climb starting point
            op_counter = 0

            for tick in range(n_ticks):
                # Beacon value for this tick (unpredictable at commit time)
                beacon_val = hash(f"beacon:{g.seed}:{tick}:{run}")

                # Commit batch: researcher chooses N points BEFORE beacon
                batch_points = []
                if current is None:
                    for _ in range(batch_size):
                        raw = [random.random() for _ in range(3)]
                        t = sum(raw)
                        f = [r / t for r in raw]
                        e = random.uniform(0, 50)
                        batch_points.append((f, e))
                else:
                    for _ in range(batch_size):
                        f, e = current
                        nf = [max(0.001, fi + random.gauss(0, 0.03))
                              for fi in f]
                        t = sum(nf)
                        nf = [fi / t for fi in nf]
                        ne = max(0, min(50, e + random.gauss(0, 3.0)))
                        batch_points.append((nf, ne))

                # Beacon reveals — evaluate all committed experiments
                # against this tick's beacon-shifted landscape
                for f, e in batch_points:
                    props = interact(f, e, g,
                                     precision=facility_precision,
                                     beacon=beacon_val)
                    v = props[1]
                    if v > best_val:
                        best_val = v
                        best_point = (f, e)

                # Learn: update current for next tick's hill-climbing
                current = best_point

            finals.append(best_val)

        mean = sum(finals) / nr
        std = (sum((v - mean)**2 for v in finals) / nr) ** 0.5
        print(f"  batch={batch_size:>3}, ticks={n_ticks:>3}: "
              f"mean={mean:.3f} ± {std:.3f}")


def test_beacon_hillclimb():
    """Hill-climbing across tick windows.

    The landscape shifts each tick (new beacon = new peak positions).
    Can a researcher still make progress across ticks by knowing the
    right neighborhood, even though the exact optimum moves?

    Compare: 1 experiment per tick (pure sequential hill-climbing across
    shifting landscape) vs base landscape (no beacon, stable peaks).
    """
    print("\n" + "=" * 70)
    print("BEACON HILL-CLIMBING: Can researchers climb across shifting ticks?")
    print("=" * 70)

    g = Galaxy(42)
    nr = 20
    budget = 300

    # Without beacon: stable landscape, smart strategy
    no_beacon = []
    for run in range(nr):
        random.seed(run * 1000)
        r = Researcher(g, 3, budget, "smart")
        r.run()
        no_beacon.append(r.best_val)

    # With beacon: landscape shifts each tick, 1 experiment per tick
    with_beacon = []
    for run in range(nr):
        random.seed(run * 1000)
        best_val = -float('inf')
        best_point = None
        current = None
        stall = 0

        for tick in range(budget):
            beacon_val = hash(f"tick:{tick}:{run}")

            if current is None:
                raw = [random.random() for _ in range(3)]
                t = sum(raw)
                pt = ([r / t for r in raw], random.uniform(0, 50))
            else:
                f, e = current
                nf = [max(0.001, fi + random.gauss(0, 0.03)) for fi in f]
                t = sum(nf)
                nf = [fi / t for fi in nf]
                ne = max(0, min(50, e + random.gauss(0, 3.0)))
                pt = (nf, ne)

            props = interact(pt[0], pt[1], g, beacon=beacon_val)
            v = props[1]

            if v > best_val:
                best_val = v
                best_point = pt
                current = pt
                stall = 0
            else:
                stall += 1

            if stall > 30:
                current = None
                stall = 0

        with_beacon.append(best_val)

    mean_nb = sum(no_beacon) / nr
    mean_wb = sum(with_beacon) / nr
    ratio = mean_wb / mean_nb if mean_nb > 0.01 else 0
    print(f"\n  No beacon (stable landscape): {mean_nb:.3f}")
    print(f"  With beacon (shifting ticks):  {mean_wb:.3f}  (ratio: {ratio:.2f})")
    print(f"\n  If ratio > 0.7: hill-climbing across ticks works")
    print(f"  If ratio < 0.3: beacon shifts destroy all learned knowledge")


def test_beacon_bruteforce():
    """The brute-force attack.

    Attacker: pre-computes optimal parameters using the BASE landscape
    (no beacon). Then submits those parameters on a tick where the
    beacon has shifted all the peaks.

    The beacon IS the landscape — without it, you're optimizing a
    different function. Pre-computed peaks are in the wrong place.
    """
    print("\n" + "=" * 70)
    print("BEACON BRUTE-FORCE: Does pre-computation help?")
    print("=" * 70)
    print("\n  Attacker optimizes base landscape (no beacon), submits on")
    print("  a tick where peaks have shifted. Honest researcher searches")
    print("  within the tick using the actual beacon landscape.\n")

    g = Galaxy(42)

    # Attacker: brute-force the BASE landscape (no beacon)
    random.seed(42)
    best_attack_val = -float('inf')
    best_attack_point = None
    for _ in range(10000):
        raw = [random.random() for _ in range(3)]
        t = sum(raw)
        f = [r / t for r in raw]
        e = random.uniform(0, 50)
        props = interact(f, e, g)  # no beacon — base peaks
        v = props[1]
        if v > best_attack_val:
            best_attack_val = v
            best_attack_point = (f, e)

    print(f"  Attacker's base-landscape best: {best_attack_val:.3f}")

    # Test across 50 beacon ticks
    attack_results = []
    honest_results = []
    for tick in range(50):
        beacon_val = hash(f"tick:{tick}")

        # Attacker: submits pre-computed point on this tick's landscape
        af, ae = best_attack_point
        props = interact(af, ae, g, beacon=beacon_val)
        attack_results.append(props[1])

        # Honest: 200 random experiments on this tick's landscape
        best_honest = -float('inf')
        random.seed(tick * 777)
        for exp in range(200):
            raw = [random.random() for _ in range(3)]
            t = sum(raw)
            f = [r / t for r in raw]
            e = random.uniform(0, 50)
            props = interact(f, e, g, beacon=beacon_val)
            if props[1] > best_honest:
                best_honest = props[1]
        honest_results.append(best_honest)

    mean_a = sum(attack_results) / len(attack_results)
    mean_h = sum(honest_results) / len(honest_results)
    std_a = (sum((v - mean_a)**2 for v in attack_results) / len(attack_results)) ** 0.5
    std_h = (sum((v - mean_h)**2 for v in honest_results) / len(honest_results)) ** 0.5

    print(f"\n  Attacker (pre-computed, 10k free evals): {mean_a:.3f} ± {std_a:.3f}")
    print(f"  Honest (200 real evals per tick):         {mean_h:.3f} ± {std_h:.3f}")
    adv = (mean_a - mean_h) / abs(mean_h) * 100 if abs(mean_h) > 0.01 else 0
    print(f"  Attacker advantage: {adv:+.1f}%")
    print()

    # Also test: attacker who knows the NEIGHBORHOOD but not the exact
    # beacon-shifted peak. They pre-compute the right region, then do
    # a small focused search within the tick.
    focused_results = []
    for tick in range(50):
        beacon_val = hash(f"tick:{tick}")
        best_focused = -float('inf')
        random.seed(tick * 888)
        # 50 experiments focused near the pre-computed peak
        af, ae = best_attack_point
        for exp in range(50):
            nf = [max(0.001, fi + random.gauss(0, 0.02)) for fi in af]
            t = sum(nf)
            nf = [fi / t for fi in nf]
            ne = max(0, min(50, ae + random.gauss(0, 2.0)))
            props = interact(nf, ne, g, beacon=beacon_val)
            if props[1] > best_focused:
                best_focused = props[1]
        focused_results.append(best_focused)

    mean_f = sum(focused_results) / len(focused_results)
    print(f"  Focused (50 evals near pre-computed peak): {mean_f:.3f}")
    adv_f = (mean_f - mean_h) / abs(mean_h) * 100 if abs(mean_h) > 0.01 else 0
    print(f"  Focused advantage over honest: {adv_f:+.1f}%")
    print(f"  (This is the value of knowing the right neighborhood)")


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

if __name__ == "__main__":
    mode = sys.argv[1] if len(sys.argv) > 1 else "basic"
    def _seed():
        return int(sys.argv[2]) if len(sys.argv) > 2 else 42

    tests = {
        "basic": test_basic, "catalyst": test_catalyst, "energy": test_energy,
        "precision": test_precision, "sharing": test_sharing,
        "reverse": test_reverse, "seeds": test_seeds,
        "elements": test_elements, "density": test_density,
        # Beacon tests
        "beacon-batches": test_beacon_batches,
        "beacon-climb": test_beacon_hillclimb,
        "beacon-bruteforce": test_beacon_bruteforce,
        # Visualization
        "landscape":  lambda: viz_landscape(_seed()),
        "properties": lambda: viz_all_properties(_seed()),
        "catalyst-compare": lambda: viz_catalyst_compare(_seed()),
        "energy-slices": lambda: viz_energy_slices(_seed()),
        "ternary":    lambda: viz_ternary(_seed()),
        "trajectory": lambda: viz_trajectory(_seed()),
        "scatter":    lambda: viz_property_scatter(_seed()),
        # All tests
        "all": lambda: [t() for t in [test_basic, test_catalyst, test_energy,
                                       test_precision, test_sharing,
                                       test_reverse, test_seeds,
                                       test_elements, test_density]],
    }
    if mode in tests:
        tests[mode]()
    else:
        print("Modes: " + " | ".join(sorted(tests)))

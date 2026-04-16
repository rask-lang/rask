"""Star placement. Mixture of Gaussians for clusters + uniform background."""

from __future__ import annotations

import math
import random
from dataclasses import dataclass


@dataclass
class Star:
    id: int
    x: float
    y: float
    z: float
    cluster: int  # -1 for background stars


def generate(
    seed: int,
    num_stars: int = 10000,
    num_clusters: int = 8,
    cluster_spread: float = 80.0,
    galaxy_radius: float = 500.0,
    background_fraction: float = 0.15,
) -> list[Star]:
    """Generate star positions using clustered placement.

    Clusters are Gaussian blobs placed within galaxy_radius.
    Background stars are uniform-random, filling the gaps.
    """
    rng = random.Random(seed)

    # Place cluster centers — spread them out, not too close together
    centers = []
    for i in range(num_clusters):
        cx = rng.gauss(0, galaxy_radius * 0.4)
        cy = rng.gauss(0, galaxy_radius * 0.4)
        cz = rng.gauss(0, galaxy_radius * 0.1)  # flattened disk
        centers.append((cx, cy, cz))

    num_background = int(num_stars * background_fraction)
    num_clustered = num_stars - num_background

    stars = []
    sid = 0

    # Clustered stars — distributed across clusters by size
    cluster_sizes = [rng.random() for _ in range(num_clusters)]
    total = sum(cluster_sizes)
    cluster_sizes = [s / total for s in cluster_sizes]

    for ci, (cx, cy, cz) in enumerate(centers):
        n = int(num_clustered * cluster_sizes[ci])
        spread = cluster_spread * (0.5 + cluster_sizes[ci])
        for _ in range(n):
            x = rng.gauss(cx, spread)
            y = rng.gauss(cy, spread)
            z = rng.gauss(cz, spread * 0.3)
            stars.append(Star(id=sid, x=x, y=y, z=z, cluster=ci))
            sid += 1

    # Fill remainder into largest cluster
    while len(stars) < num_clustered:
        ci = cluster_sizes.index(max(cluster_sizes))
        cx, cy, cz = centers[ci]
        spread = cluster_spread * (0.5 + cluster_sizes[ci])
        x = rng.gauss(cx, spread)
        y = rng.gauss(cy, spread)
        z = rng.gauss(cz, spread * 0.3)
        stars.append(Star(id=sid, x=x, y=y, z=z, cluster=ci))
        sid += 1

    # Background stars — uniform in a sphere
    for _ in range(num_background):
        r = galaxy_radius * rng.random() ** (1.0 / 3.0)
        theta = rng.uniform(0, 2 * math.pi)
        phi = math.acos(2 * rng.random() - 1)
        x = r * math.sin(phi) * math.cos(theta)
        y = r * math.sin(phi) * math.sin(theta)
        z = r * math.cos(phi) * 0.3  # flatten
        stars.append(Star(id=sid, x=x, y=y, z=z, cluster=-1))
        sid += 1

    return stars


def distance(a: Star, b: Star) -> float:
    dx = a.x - b.x
    dy = a.y - b.y
    dz = a.z - b.z
    return math.sqrt(dx * dx + dy * dy + dz * dz)

"""Graph analysis on star positions. Connectivity, clusters, bridges."""

from __future__ import annotations

from collections import deque
from dataclasses import dataclass

from .generate import Star, distance


@dataclass
class Graph:
    num_nodes: int
    adj: dict[int, list[int]]  # adjacency list


def build_graph(stars: list[Star], jump_range: float) -> Graph:
    """Build adjacency from distance threshold. O(n^2) — fine for 10K stars."""
    adj: dict[int, list[int]] = {s.id: [] for s in stars}
    for i, a in enumerate(stars):
        for b in stars[i + 1:]:
            if distance(a, b) <= jump_range:
                adj[a.id].append(b.id)
                adj[b.id].append(a.id)
    return Graph(num_nodes=len(stars), adj=adj)


def connected_components(graph: Graph) -> list[list[int]]:
    """BFS-based connected components."""
    visited = set()
    components = []
    for node in graph.adj:
        if node in visited:
            continue
        comp = []
        queue = deque([node])
        visited.add(node)
        while queue:
            n = queue.popleft()
            comp.append(n)
            for nb in graph.adj[n]:
                if nb not in visited:
                    visited.add(nb)
                    queue.append(nb)
        components.append(comp)
    return components


@dataclass
class TopologyStats:
    num_stars: int
    num_edges: int
    num_components: int
    largest_component: int
    isolated_stars: int  # degree 0
    avg_degree: float
    max_degree: int
    component_sizes: list[int]


def analyze(graph: Graph) -> TopologyStats:
    degrees = {n: len(nbs) for n, nbs in graph.adj.items()}
    num_edges = sum(degrees.values()) // 2
    comps = connected_components(graph)
    comp_sizes = sorted([len(c) for c in comps], reverse=True)

    return TopologyStats(
        num_stars=graph.num_nodes,
        num_edges=num_edges,
        num_components=len(comps),
        largest_component=comp_sizes[0] if comp_sizes else 0,
        isolated_stars=sum(1 for d in degrees.values() if d == 0),
        avg_degree=sum(degrees.values()) / len(degrees) if degrees else 0,
        max_degree=max(degrees.values()) if degrees else 0,
        component_sizes=comp_sizes,
    )


def find_bridges(stars: list[Star], graph: Graph, top_n: int = 20) -> list[int]:
    """Find stars with highest degree — likely bridge candidates.

    True betweenness centrality is O(V*E), too expensive for 10K stars.
    High-degree nodes between clusters are a cheap proxy.
    """
    degrees = [(len(graph.adj[s.id]), s.id) for s in stars]
    degrees.sort(reverse=True)
    return [sid for _, sid in degrees[:top_n]]

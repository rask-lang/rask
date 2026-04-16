"""Matplotlib wrappers. Each function takes data, saves a PNG. No interactive plots."""

from __future__ import annotations

from pathlib import Path
from typing import Sequence


def _ensure_dir(path: str):
    Path(path).parent.mkdir(parents=True, exist_ok=True)


def time_series(
    series: dict[str, list[tuple[int, float]]],
    path: str,
    title: str = "",
    xlabel: str = "tick",
    ylabel: str = "",
):
    import matplotlib.pyplot as plt

    fig, ax = plt.subplots(figsize=(10, 6))
    for label, data in series.items():
        ticks, values = zip(*data) if data else ([], [])
        ax.plot(ticks, values, label=label)
    if title:
        ax.set_title(title)
    ax.set_xlabel(xlabel)
    if ylabel:
        ax.set_ylabel(ylabel)
    if len(series) > 1:
        ax.legend()
    _ensure_dir(path)
    fig.savefig(path, dpi=150, bbox_inches="tight")
    plt.close(fig)


def heatmap(
    grid: list[list[float]],
    path: str,
    title: str = "",
    xlabel: str = "",
    ylabel: str = "",
):
    import matplotlib.pyplot as plt
    import matplotlib.colors as mcolors

    fig, ax = plt.subplots(figsize=(10, 8))
    im = ax.imshow(grid, aspect="auto", origin="lower")
    fig.colorbar(im, ax=ax)
    if title:
        ax.set_title(title)
    if xlabel:
        ax.set_xlabel(xlabel)
    if ylabel:
        ax.set_ylabel(ylabel)
    _ensure_dir(path)
    fig.savefig(path, dpi=150, bbox_inches="tight")
    plt.close(fig)


def histogram(
    values: Sequence[float],
    path: str,
    bins: int = 50,
    title: str = "",
    xlabel: str = "",
):
    import matplotlib.pyplot as plt

    fig, ax = plt.subplots(figsize=(8, 5))
    ax.hist(values, bins=bins)
    if title:
        ax.set_title(title)
    if xlabel:
        ax.set_xlabel(xlabel)
    ax.set_ylabel("count")
    _ensure_dir(path)
    fig.savefig(path, dpi=150, bbox_inches="tight")
    plt.close(fig)


def scatter(
    xs: Sequence[float],
    ys: Sequence[float],
    path: str,
    title: str = "",
    xlabel: str = "",
    ylabel: str = "",
    colors: Sequence[float] | None = None,
    size: float = 5,
):
    import matplotlib.pyplot as plt

    fig, ax = plt.subplots(figsize=(10, 8))
    sc = ax.scatter(xs, ys, c=colors, s=size, alpha=0.7)
    if colors is not None:
        fig.colorbar(sc, ax=ax)
    if title:
        ax.set_title(title)
    if xlabel:
        ax.set_xlabel(xlabel)
    if ylabel:
        ax.set_ylabel(ylabel)
    _ensure_dir(path)
    fig.savefig(path, dpi=150, bbox_inches="tight")
    plt.close(fig)


def scatter_3d(
    xs: Sequence[float],
    ys: Sequence[float],
    zs: Sequence[float],
    path: str,
    title: str = "",
    xlabel: str = "",
    ylabel: str = "",
    zlabel: str = "",
    colors: Sequence[float] | None = None,
    size: float = 2,
):
    import matplotlib.pyplot as plt

    fig = plt.figure(figsize=(12, 10))
    ax = fig.add_subplot(111, projection="3d")
    ax.scatter(xs, ys, zs, c=colors, s=size, alpha=0.5)
    if title:
        ax.set_title(title)
    if xlabel:
        ax.set_xlabel(xlabel)
    if ylabel:
        ax.set_ylabel(ylabel)
    if zlabel:
        ax.set_zlabel(zlabel)
    _ensure_dir(path)
    fig.savefig(path, dpi=150, bbox_inches="tight")
    plt.close(fig)


def multi_series(
    series_groups: dict[str, dict[str, list[tuple[int, float]]]],
    path: str,
    title: str = "",
    xlabel: str = "tick",
):
    """Multiple subplots, one per group. For comparing metrics side by side."""
    import matplotlib.pyplot as plt

    n = len(series_groups)
    fig, axes = plt.subplots(n, 1, figsize=(10, 4 * n), sharex=True)
    if n == 1:
        axes = [axes]
    for ax, (group_name, series) in zip(axes, series_groups.items()):
        for label, data in series.items():
            ticks, values = zip(*data) if data else ([], [])
            ax.plot(ticks, values, label=label)
        ax.set_ylabel(group_name)
        if len(series) > 1:
            ax.legend(fontsize="small")
    axes[-1].set_xlabel(xlabel)
    if title:
        axes[0].set_title(title)
    _ensure_dir(path)
    fig.savefig(path, dpi=150, bbox_inches="tight")
    plt.close(fig)

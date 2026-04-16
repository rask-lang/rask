"""Metric recording. Append numbers per tick, export to CSV or JSON."""

from __future__ import annotations

import csv
import json
import io
from dataclasses import dataclass, field


@dataclass
class Recorder:
    entries: list[tuple[int, str, float]] = field(default_factory=list)

    def add(self, tick: int, key: str, value: float):
        self.entries.append((tick, key, value))

    def series(self, key: str) -> list[tuple[int, float]]:
        return [(t, v) for t, k, v in self.entries if k == key]

    def keys(self) -> set[str]:
        return {k for _, k, _ in self.entries}

    def snapshot(self, tick: int) -> dict[str, float]:
        return {k: v for t, k, v in self.entries if t == tick}

    def last(self, key: str) -> float | None:
        for t, k, v in reversed(self.entries):
            if k == key:
                return v
        return None

    def to_csv(self, path: str):
        with open(path, "w", newline="") as f:
            w = csv.writer(f)
            w.writerow(["tick", "key", "value"])
            w.writerows(self.entries)

    def to_json(self, path: str):
        by_key: dict[str, list[tuple[int, float]]] = {}
        for t, k, v in self.entries:
            by_key.setdefault(k, []).append([t, v])
        with open(path, "w") as f:
            json.dump(by_key, f)

    def csv_string(self) -> str:
        buf = io.StringIO()
        w = csv.writer(buf)
        w.writerow(["tick", "key", "value"])
        w.writerows(self.entries)
        return buf.getvalue()

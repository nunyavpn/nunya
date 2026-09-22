#!/usr/bin/env python3
"""Traces the app's mark out of design/nonya.png as path data for `MARK` in src/views/icons.ts.

The artwork is a painting: a navy tile, a bright arch, dark rings receding into a tunnel, and a
bright road winding out of it. At 18pt in a menu bar only its silhouette survives, so this keeps
the two bright shapes — the arch and the road — and drops the rest:

  1. a pixel is part of the mark when its brightest channel is over THRESHOLD, which separates the
     arch and the road from the ground and from the dimmer inner rings (lower thresholds let
     ragged fragments of those rings in);
  2. each connected shape bigger than MIN_AREA is outlined (Moore-neighbour tracing);
  3. the outlines are simplified (Ramer-Douglas-Peucker, EPSILON pixels of the 1254px artwork —
     well under a tenth of a pixel at 36px);
  4. the mark is fitted to 22 of the icon grid's 24 units, centred.

Prints one subpath per line. Needs Pillow and NumPy (`pip install pillow numpy`); it runs only when
the artwork changes, like `npm run tauri icon`, so neither is a dependency of the app.

    python3 scripts/trace-mark.py
"""

from collections import deque
from pathlib import Path

import numpy as np
from PIL import Image

SOURCE = Path(__file__).resolve().parent.parent / "design" / "nonya.png"
THRESHOLD = 200
MIN_AREA = 2000
EPSILON = 2.2
GRID = 24.0
FILL = 22.0

NEIGHBOURS = [(1, 0), (1, 1), (0, 1), (-1, 1), (-1, 0), (-1, -1), (0, -1), (1, -1)]


def shapes(mask):
    """Labels of the connected shapes in `mask` bigger than MIN_AREA, largest first."""
    labels = np.zeros(mask.shape, dtype=np.int32)
    height, width = mask.shape
    found = []
    for y in range(height):
        for x in np.nonzero(mask[y] & (labels[y] == 0))[0]:
            if labels[y, x]:
                continue
            label = len(found) + 1
            labels[y, x] = label
            queue, area = deque([(y, x)]), 0
            while queue:
                cy, cx = queue.popleft()
                area += 1
                for ny, nx in ((cy + 1, cx), (cy - 1, cx), (cy, cx + 1), (cy, cx - 1)):
                    if 0 <= ny < height and 0 <= nx < width and mask[ny, nx] and not labels[ny, nx]:
                        labels[ny, nx] = label
                        queue.append((ny, nx))
            found.append((area, label))
    return labels, [label for area, label in sorted(found, reverse=True) if area > MIN_AREA]


def outline(inside):
    """The boundary of one shape, clockwise from its topmost-leftmost pixel."""
    height, width = inside.shape
    ys, xs = np.nonzero(inside)
    start = (int(xs[ys == ys.min()].min()), int(ys.min()))
    points, current, back = [start], start, 6
    while True:
        for i in range(8):
            d = (back + 1 + i) % 8
            nx, ny = current[0] + NEIGHBOURS[d][0], current[1] + NEIGHBOURS[d][1]
            if 0 <= nx < width and 0 <= ny < height and inside[ny, nx]:
                back, current = (d + 4) % 8, (nx, ny)
                break
        if current == start:
            return points
        points.append(current)


def simplify(points, epsilon):
    points = np.asarray(points, float)

    def keep(a, b):
        if b - a < 2:
            return [a, b]
        p, q = points[a], points[b]
        dx, dy = q - p
        length = np.hypot(dx, dy) or 1.0
        segment = points[a : b + 1]
        distance = np.abs(dx * (segment[:, 1] - p[1]) - dy * (segment[:, 0] - p[0])) / length
        i = int(np.argmax(distance))
        if distance[i] <= epsilon:
            return [a, b]
        return keep(a, a + i)[:-1] + keep(a + i, b)

    return points[keep(0, len(points) - 1)]


def main():
    art = np.asarray(Image.open(SOURCE).convert("RGB"))
    labels, kept = shapes(art.max(axis=2) > THRESHOLD)
    outlines = [simplify(outline(labels == label), EPSILON) for label in kept]

    every = np.vstack(outlines)
    (x0, y0), (x1, y1) = every.min(axis=0), every.max(axis=0)
    scale = FILL / max(x1 - x0, y1 - y0)
    dx = GRID / 2 - (x0 + x1) / 2 * scale
    dy = GRID / 2 - (y0 + y1) / 2 * scale

    for points in outlines:
        coords = [f"{x * scale + dx:.2f} {y * scale + dy:.2f}" for x, y in points]
        print(f"M{coords[0]}L{' '.join(coords[1:])}Z")


if __name__ == "__main__":
    main()

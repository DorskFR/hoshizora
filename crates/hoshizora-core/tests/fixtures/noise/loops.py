import requests


def fetch_status(url):
    for attempt in range(3):
        r = requests.get(url)
        if r.ok:
            return r


def crawl(urls):
    for u in urls:
        requests.get(u)


def cells(rows):
    out = []
    for row in rows:
        out.append([c.strip() for c in row.cells])
    return out


def read_tips(paths):
    return [p.strip() for p in paths]


def all_tips(paths):
    return [t.upper() for t in read_tips(paths)]


def overlaps(boxes):
    out = []
    for a in boxes:
        for b in boxes:
            if a != b and a[0] == b[0]:
                out.append((a, b))
    return out


def nearest_free(grid, r):
    for dy in range(-r, r + 1):
        for dx in range(-r, r + 1):
            if grid.free(dx, dy):
                return dx, dy


def seasons(root):
    out = []
    for show in sorted(p for p in root.iterdir() if p.is_dir()):
        for season in sorted(p for p in show.iterdir() if p.is_dir()):
            out.append(season)
    return out


CAP = 8


def cutoff(series, ctx, batch):
    out = []
    for s in series:
        for g in ctx.groups_of(s.id):
            for item in batch.items_of(g.id):
                out.append(item)
    return out


def around(grid, cx, cy, r):
    hits = 0
    for y in range(cy - r, cy + r + 1):
        for x in range(cx - r, cx + r + 1):
            for i in grid.get((y, x), ()):
                hits += i
    return hits


def capped(text, steps):
    at = [[] for _ in text]
    for i in range(len(text)):
        for step in steps(text, i):
            for prefix in at[i]:
                if len(at[step]) >= CAP:
                    break
                at[step].append(prefix)
    return at


def lines_of(paths):
    out = []
    for p in paths:
        with open(p) as f:
            for line in f:
                out.append([c for c in line.split(",") if c])
    return out


GAP = 3


def near_matches(words, keys):
    hits = 0
    for at in range(len(words)):
        limit = at if not keys else min(len(words) - 1, at + GAP)
        for k in keys:
            if any(words[i] == k for i in range(at, limit + 1)):
                hits += 1
    return hits


def pair_legs(stops, route):
    out = []
    for a in stops:
        for b in stops:
            legs = [route(a, b)] + ([route(b, a)] if a != b else [])
            for leg in legs:
                out.append(leg)
    return out


def ring_hits(lines, points, rings=(1.0, 2.0, 4.0)):
    hits = 0
    for line in lines:
        for r in rings:
            for p in points:
                if line.near(p, r):
                    hits += 1
    return hits


def by_role(actors, roles_of, clips_of):
    out = []
    for actor in actors:
        for role in roles_of[actor]:
            for clip in clips_of[role]:
                out.append((actor, clip))
    return out


def run_edges(rows):
    n = 0
    for row in rows:
        edges = [c for c in row if c > 0]
        for a in edges:
            for b in edges:
                n += a * b
    return n


def later_pairs(xs):
    n = 0
    for i in range(len(xs)):
        for y in xs[i + 1 :]:
            n += xs[i] * y
    return n


def against_seen(xs):
    seen = [xs[0]]
    n = 0
    for x in xs:
        for s in seen:
            n += x * s
        seen.append(x)
    return n


def halves(xs):
    if len(xs) <= 1:
        return [xs]
    left, right = xs[: len(xs) // 2], xs[len(xs) // 2 :]
    return halves(left) + halves(right)

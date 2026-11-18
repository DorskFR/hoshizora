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

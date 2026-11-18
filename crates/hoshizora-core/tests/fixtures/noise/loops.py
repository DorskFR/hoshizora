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

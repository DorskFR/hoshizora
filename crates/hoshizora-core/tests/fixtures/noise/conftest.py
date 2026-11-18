import requests


def warm(urls):
    for url in urls:
        requests.get(url)

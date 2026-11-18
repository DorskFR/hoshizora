import numpy as np
import requests


class Runner:
    def cross(self, url):
        return requests.get(url)


def normals(faces):
    out = []
    for a, b in faces:
        out.append(np.cross(a, b))
    return out

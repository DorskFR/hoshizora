import functools
import threading

import typer

from app import tasks as jobs
from app.util import run_command

app = typer.Typer()


class Box:
    @property
    def area(self):
        return 1


@app.command()
def go(xs: list[int]) -> None:
    jobs.work()
    threading.Thread(target=serve).start()
    print(sorted(xs, key=rank), Box().area)
    run("ls")
    outer()()
    lazy()
    is_mob(1)
    fan_out([])


def serve() -> None:
    pass


def rank(x: int) -> int:
    return x


def run(cmd: str, runner=run_command) -> None:
    runner(cmd)


def outer():
    def inner():
        return leaf()

    return inner


def leaf() -> int:
    return 1


def lazy() -> None:
    if True:
        from app.extra import helper as h

        h()


@functools.lru_cache
def cached() -> int:
    return 1


def truly_dead() -> None:
    pass


class Mob:
    def on_pet(self) -> bool:
        return True


def is_mob(x) -> bool:
    return isinstance(x, Mob)


def fan_out(pages) -> None:
    from . import tasks

    with open("x") as fh:

        def first(page):
            return tasks.work_page(page), fh

        list(map(first, pages))

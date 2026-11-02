from enum import Enum, auto


class Status(str, Enum):
    ACTIVE = "active"
    PAUSED = "paused"
    DONE = auto()


class Plain:
    A = 1
    B = 2


def label(status):
    match status:
        case Status.ACTIVE | Status.PAUSED:
            return "a"
        case "x":
            return "x"
        case _:
            return None


def color(status):
    if status == Status.ACTIVE:
        return "green"
    elif status in ("paused", "done"):
        return "grey"
    else:
        raise ValueError(status)


def guarded(status):
    match status:
        case Status.ACTIVE:
            return 1
        case other if other:
            return 2

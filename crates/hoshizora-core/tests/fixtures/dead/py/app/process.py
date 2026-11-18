class Stage:
    def __init__(self, name, run):
        self.name = name
        self.run = run


def retain() -> None:
    from app.retain import retain as prune

    prune()


STAGES = (Stage("retain", retain),)

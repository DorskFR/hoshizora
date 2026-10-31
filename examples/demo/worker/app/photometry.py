import math
from functools import wraps

from opentelemetry import trace

tracer = trace.get_tracer("observatory-worker")

PARSEC_PER_LY = 1 / 3.2616


def traced(fn):
    """Span per call with code.* attributes so hoshizora joins it to the function node."""
    code = fn.__code__

    @wraps(fn)
    def wrapper(*args, **kwargs):
        with tracer.start_as_current_span(
            fn.__qualname__,
            attributes={
                "code.function.name": f"{fn.__module__}.{fn.__qualname__}",
                "code.file.path": code.co_filename,
                "code.line.number": code.co_firstlineno,
            },
        ):
            return fn(*args, **kwargs)

    return wrapper


@traced
def apparent_magnitude(flux: float, distance_ly: float) -> float:
    """CPU-heavy hot function: iterative refinement that is called once per star."""
    parsecs = distance_ly * PARSEC_PER_LY
    absolute = -2.5 * math.log10(max(flux, 1e-9))
    estimate = absolute
    for _ in range(20_000):
        estimate = absolute + 5 * math.log10(parsecs) - 5 + math.sin(estimate) * 1e-9
    return round(estimate, 4)


@traced
def compute_all(stars: list[dict]) -> list[dict]:
    return [
        {"id": s["id"], "magnitude": apparent_magnitude(s["flux"], s["distance_ly"])}
        for s in stars
    ]

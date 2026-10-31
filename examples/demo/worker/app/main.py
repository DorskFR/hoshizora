import asyncio
import random

from fastapi import FastAPI
from pydantic import BaseModel

from app import db, telemetry
from app.photometry import compute_all

app = FastAPI(title="observatory-worker")
tracer = telemetry.init(app)


class Star(BaseModel):
    id: int
    name: str
    constellation: str
    distance_ly: float
    flux: float
    magnitude: float | None = None


class MagnitudeRequest(BaseModel):
    stars: list[Star]


@app.get("/health")
async def health():
    return {"status": "ok"}


async def sky_conditions(site: str) -> str:
    with tracer.start_as_current_span("sky_conditions"):
        await asyncio.sleep(0.03)
        return random.choice(["clear", "clear", "hazy", "cloudy"])


async def seeing_index(site: str) -> float:
    with tracer.start_as_current_span("seeing_index"):
        recent = await db.recent_observations(site)
        if not recent:
            return 1.0
        return round(sum(r["seeing"] for r in recent) / len(recent), 2)


@app.get("/weather/{site}")
async def weather(site: str):
    sky, seeing = await asyncio.gather(sky_conditions(site), seeing_index(site))
    return {"site": site, "sky": sky, "seeing": seeing}


@app.get("/inventory")
async def inventory():
    return {"inventory": await db.inventory()}


@app.post("/compute/magnitudes")
async def compute_magnitudes(req: MagnitudeRequest):
    magnitudes = compute_all([s.model_dump() for s in req.stars])
    await db.publish_event("catalog.recomputed", {"count": len(magnitudes)})
    return {"magnitudes": magnitudes}

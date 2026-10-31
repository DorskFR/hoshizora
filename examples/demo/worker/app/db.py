import json
import os

import psycopg
from opentelemetry import trace
from psycopg.rows import dict_row

tracer = trace.get_tracer("observatory-worker")

DATABASE_URL = os.environ.get(
    "DATABASE_URL", "postgres://observatory:observatory@localhost:5432/observatory"
)


async def connect() -> psycopg.AsyncConnection:
    return await psycopg.AsyncConnection.connect(DATABASE_URL, row_factory=dict_row, autocommit=True)


async def recent_observations(site: str, limit: int = 5) -> list[dict]:
    async with await connect() as conn:
        cur = await conn.execute(
            "SELECT star_id, seeing, observed_at FROM observations "
            "WHERE site = %s ORDER BY observed_at DESC LIMIT %s",
            (site, limit),
        )
        return await cur.fetchall()


async def inventory() -> list[dict]:
    async with await connect() as conn:
        cur = await conn.execute("SELECT instrument, available FROM inventory ORDER BY instrument")
        return await cur.fetchall()


async def publish_event(topic: str, payload: dict) -> None:
    with tracer.start_as_current_span(
        f"publish {topic}",
        kind=trace.SpanKind.PRODUCER,
        attributes={
            "messaging.system": "postgresql",
            "messaging.operation.type": "publish",
            "messaging.destination.name": topic,
        },
    ):
        async with await connect() as conn:
            await conn.execute(
                "INSERT INTO events (topic, payload) VALUES (%s, %s)",
                (topic, json.dumps(payload)),
            )
            await conn.execute("SELECT pg_notify(%s, %s)", (topic, json.dumps(payload)))

"""Users API."""

import asyncio
import httpx
from fastapi import FastAPI, Depends
from .db import get_session as session_dep, Repo

app = FastAPI()
TIMEOUT = 5.0


class UserService(Repo):
    """Reads users from the upstream service."""

    def __init__(self, client: httpx.AsyncClient, retries: int = 3):
        self.client = client
        self.retries = retries

    async def fetch_one(self, user_id: int) -> dict:
        """Fetch a single user.

        Retries on failure.
        """
        for attempt in range(self.retries):
            try:
                resp = await self.client.get(f"/users/{user_id}")
                return resp.json()
            except httpx.HTTPError:
                if attempt == self.retries - 1:
                    raise
        return {}

    def _normalise(self, raw):
        return {k.lower(): v for k, v in raw.items()}


@app.get("/users")
async def list_users(ids: list[int], svc: UserService = Depends(session_dep)) -> list[dict]:
    """List users concurrently."""
    results = await asyncio.gather(*(svc.fetch_one(i) for i in ids))
    cleaned = [svc._normalise(r) for r in results if r]

    def key(item):
        return item.get("name", "")

    return sorted(cleaned, key=key) or []


app.add_api_route("/users", list_users, methods=["GET"])


def legacy(base: str, user_id: int):
    httpx.get("{}/users/{id}".format(base, id=user_id))
    httpx.get("%s/users/%d" % (base, user_id))
    return (httpx
        .Client()
        .get(base + "/health"))


handler = session_dep(list_users)
not_callable = httpx.Client(timeout=TIMEOUT)

if __name__ == "__main__":
    import uvicorn

    uvicorn.run(app, port=8000)

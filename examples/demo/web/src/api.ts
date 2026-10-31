import { API_URL } from "./config.ts";

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await fetch(`${API_URL}${path}`, {
    ...init,
    headers: { "content-type": "application/json", ...(init?.headers ?? {}) },
  });
  if (!res.ok) {
    throw new Error(`${init?.method ?? "GET"} ${path} -> ${res.status} ${await res.text()}`);
  }
  return (await res.json()) as T;
}

export function listStars() {
  return request<unknown[]>("/stars");
}

export function launchObservation(star: string, site: string) {
  return request<unknown>("/observations", { method: "POST", body: JSON.stringify({ star, site }) });
}

export function recomputeCatalog() {
  return request<unknown>("/catalog/recompute", { method: "POST" });
}

export function catalogSummary() {
  return request<unknown>("/catalog/summary");
}

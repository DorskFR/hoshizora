import { mkdir, writeFile, readFile } from "node:fs/promises";

export async function store(dir: string, file: string, data: string) {
  await mkdir(dir, { recursive: true });
  await writeFile(file, data);
}

export async function reload(api: { post: (u: string) => Promise<void> }, client: { get: (u: string) => Promise<string> }) {
  await api.post("/api/sync");
  return await client.get("/api/status");
}

export async function both(client: { get: (u: string) => Promise<string> }, api: { get: (u: string) => Promise<string> }, path: string) {
  const user = await client.get("/api/user");
  const cfg = await readFile(path, "utf8");
  const plan = await api.get("/api/plan");
  return [user, cfg, plan];
}

export async function withLocal(client: { get: (u: string) => Promise<string> }, path: string) {
  const user = await client.get("/api/user");
  const cfg = await readFile(path, "utf8");
  return [user, cfg];
}

export async function resync(api: { syncMail: (a: string) => Promise<void> }, client: { get: (u: string) => Promise<string> }) {
  await api.syncMail("inbox");
  return await client.get("/api/mail");
}

export async function localOnly(path: string, dir: string) {
  const cfg = await readFile(path, "utf8");
  await mkdir(dir, { recursive: true });
  return cfg;
}

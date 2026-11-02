import { Pool } from "pg";

const pool = new Pool();

export async function start(id: string): Promise<void> {
  await pool.query("UPDATE jobs SET status = 'active', started_at = now() WHERE id = $1 AND status = 'paused'", [id]);
}

export async function finish(id: string): Promise<void> {
  await pool.query("UPDATE jobs SET status = 'done' WHERE id = $1", [id]);
}

export async function enqueue(id: string): Promise<void> {
  await pool.query("INSERT INTO jobs (id, status, kind) VALUES ($1, 'paused', 'render')", [id]);
}

export async function live(): Promise<unknown> {
  return pool.query("SELECT id FROM jobs WHERE status IN ('active', 'paused')");
}

export async function byUser(user: string): Promise<unknown> {
  return pool.query("SELECT j.id FROM jobs j JOIN users u ON u.id = j.user_id WHERE u.name = $1 AND j.status = 'done'", [user]);
}

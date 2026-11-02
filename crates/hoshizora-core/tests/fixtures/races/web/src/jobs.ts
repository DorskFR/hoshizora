import { Pool } from "pg";

const pool = new Pool();

export async function requeue(id: string): Promise<void> {
  const row = await pool.query("SELECT status FROM jobs WHERE id = $1", [id]);
  if (row.rows[0].status === "failed") {
    await pool.query("UPDATE jobs SET status = 'queued' WHERE id = $1", [id]);
  }
}

export async function requeueInTx(id: string): Promise<void> {
  await pool.query("BEGIN");
  await pool.query("SELECT status FROM jobs WHERE id = $1", [id]);
  await pool.query("UPDATE jobs SET status = 'queued' WHERE id = $1", [id]);
  await pool.query("COMMIT");
}

export function syncLookAlike(id: string): void {
  pool.query("SELECT status FROM jobs WHERE id = $1", [id]);
  pool.query("UPDATE jobs SET status = 'queued' WHERE id = $1", [id]);
}

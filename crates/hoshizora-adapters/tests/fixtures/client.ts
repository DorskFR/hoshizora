import { writable } from "svelte/store";
import type { User } from "./types";
import * as log from "./log";

const BASE_URL = "/api";

export interface UserRepo extends Base {
  list(): Promise<User[]>;
}

/**
 * Fetches users from the API.
 * @param ids user ids
 */
export async function fetchUsers(ids: number[], signal?: AbortSignal): Promise<User[]> {
  const out: User[] = [];
  for (const id of ids) {
    const res = await fetch(`${BASE_URL}/users/${id}`, { signal });
    if (!res.ok) {
      throw new Error("failed");
    }
    out.push(await res.json());
  }
  return out.filter((u) => u.active && u.name);
}

/** Debounced save. */
export const saveUser = async (user: User, retries = 2): Promise<void> => {
  await fetch("/api/users", { method: "POST", body: JSON.stringify(user) });
};

export default class UserClient implements UserRepo {
  private cache = new Map<number, User>();

  constructor(private readonly base: string) {}

  /** List all users. */
  async list(): Promise<User[]> {
    const users = await fetchUsers([1, 2]);
    users.forEach((u) => this.cache.set(u.id, u));
    return users;
  }

  @memoize()
  get(id: number): User | undefined {
    return this.cache.get(id) ?? undefined;
  }
}

function helper(x: number) {
  return x > 0 ? x : -x;
}

export function legacy(id: number) {
  fetch(BASE_URL + "/users/" + id + "/posts");
  fetch(id > 0 ? "/a" : "/b", (window.opts || {}) as RequestInit);
  fetch(BASE_URL || "/fallback");
  return client
    .get("/a")
    .then((r) => r.json());
}

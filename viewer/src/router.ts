export type View = "home" | "map" | "story";

export interface Route {
  /** `null` targets the server's unprefixed API (single-project servers). */
  project: string | null;
  view: View;
  node: string | null;
}

export const HOME: Route = { project: null, view: "home", node: null };

/**
 * `#/` home · `#/p/{project}` map · `#/p/{project}/node/{id}` map with a selection ·
 * `#/p/{project}/story/{id}` story. Without `/p/{project}` the same paths target the unprefixed API.
 */
export function parseRoute(hash: string): Route {
  const path = hash.replace(/^#/, "").replace(/^\/+/, "");
  if (!path) return HOME;
  const parts = path.split("/");
  let project: string | null = null;
  if (parts[0] === "p" && parts[1]) {
    project = safeDecode(parts[1]);
    parts.splice(0, 2);
  }
  const [view, ...rest] = parts;
  const node = rest.length ? safeDecode(rest.join("/")) : null;
  if (view === "story" && node) return { project, view: "story", node };
  if (view === "node" && node) return { project, view: "map", node };
  if (view === "map" || project !== null) return { project, view: "map", node: null };
  return HOME;
}

export function formatRoute(r: Route): string {
  const prefix = r.project !== null ? `#/p/${encodeURIComponent(r.project)}` : "#";
  if (r.view === "home") return "#/";
  if (r.view === "story" && r.node) return `${prefix}/story/${encodeURIComponent(r.node)}`;
  if (r.node) return `${prefix}/node/${encodeURIComponent(r.node)}`;
  return r.project !== null ? prefix : `${prefix}/map`;
}

/** API path prefix for a project: `/api/projects/{p}` or `/api` for the unprefixed form. */
export function apiBase(project: string | null): string {
  return project === null ? "/api" : `/api/projects/${encodeURIComponent(project)}`;
}

function safeDecode(s: string): string {
  try {
    return decodeURIComponent(s);
  } catch {
    return s;
  }
}

export const ADMIN_ROOT = "/admin";
export const API_DOCS_PATH = "/docs";

export function adminPath(path = ""): string {
  if (!path || path === "/") return ADMIN_ROOT;
  return `${ADMIN_ROOT}${path.startsWith("/") ? path : `/${path}`}`;
}

export function publicObjectPath(iri: string): string {
  return `/objects/view/${encodeURIComponent(iri)}`;
}

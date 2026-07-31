export class PortalApiError extends Error {
  status: number;
  code?: string;

  constructor(status: number, message: string, code?: string) {
    super(message);
    this.status = status;
    this.code = code;
  }
}

export interface InstanceInfo {
  name: string;
  instance_url: string;
  uri_prefix: string;
  front_page_text: string;
  setup_required: boolean;
  policies: {
    allow_public_signup: boolean;
    require_login: boolean;
  };
  capabilities: {
    browser_sessions: boolean;
    legacy_api: boolean;
    structured_search: boolean;
    sequence_search: boolean;
    data_lab: boolean;
    sql_console: boolean;
  };
}

export interface SessionUser {
  id: string;
  username: string;
  name: string;
  email: string;
  affiliation: string | null;
  graph_uri: string;
  is_admin: boolean;
  is_curator: boolean;
  is_member: boolean;
  created_at: string;
  updated_at: string;
}

export interface SessionInfo {
  authenticated: boolean;
  user: SessionUser | null;
}

export interface PortalSearchHit {
  uri: string;
  display_id: string | null;
  version: string | null;
  name: string | null;
  description: string | null;
  object_type: string | null;
}

export interface PortalSearchResponse {
  items: PortalSearchHit[];
  total: number;
  offset: number;
  limit: number;
}

export interface PortalObject {
  id: string;
  iri: string;
  sbol_class: string;
  display_id?: string | null;
  name?: string | null;
  description?: string | null;
  graph_id?: string | null;
  types: string[];
  roles: string[];
  data: Record<string, unknown>;
  content_hash: number[];
}

export interface SetupRequest {
  instanceName: string;
  instanceUrl: string;
  uriPrefix: string;
  frontPageText: string;
  allowPublicSignup: boolean;
  requireLogin: boolean;
  userName: string;
  userFullName: string;
  userEmail: string;
  affiliation?: string;
  userPassword: string;
  userPasswordConfirm: string;
}

export interface RegistrationRequest {
  name: string;
  username: string;
  email: string;
  affiliation?: string;
  password: string;
}

export async function fetchInstance(
  signal?: AbortSignal
): Promise<InstanceInfo> {
  return requestJson<InstanceInfo>("/api/v2/instance", { signal });
}

export async function fetchSession(signal?: AbortSignal): Promise<SessionInfo> {
  return requestJson<SessionInfo>("/api/v2/session", { signal });
}

export async function createSession(
  identifier: string,
  password: string
): Promise<SessionInfo> {
  return requestJson<SessionInfo>("/api/v2/session", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ identifier, password }),
  });
}

export async function deleteSession(): Promise<void> {
  const response = await fetch("/api/v2/session", { method: "DELETE" });
  if (!response.ok) throw await responseError(response);
}

export async function searchPortal(
  query: { q?: string; type?: string; offset?: number; limit?: number },
  signal?: AbortSignal
): Promise<PortalSearchResponse> {
  const params = new URLSearchParams();
  if (query.q) params.set("q", query.q);
  if (query.type) params.set("type", query.type);
  if (query.offset) params.set("offset", String(query.offset));
  if (query.limit) params.set("limit", String(query.limit));
  return requestJson<PortalSearchResponse>(
    `/api/v2/search${params.size > 0 ? `?${params}` : ""}`,
    { signal }
  );
}

export async function fetchPortalObject(
  iri: string,
  signal?: AbortSignal
): Promise<PortalObject> {
  return requestJson<PortalObject>(
    `/api/v2/objects/${encodeURIComponent(iri)}`,
    { signal, headers: { Accept: "application/json" } }
  );
}

export async function setupInstance(request: SetupRequest): Promise<void> {
  await requestEmpty("/setup", {
    method: "POST",
    headers: {
      Accept: "application/json",
      "Content-Type": "application/json",
    },
    body: JSON.stringify(request),
  });
}

export async function registerAccount(
  request: RegistrationRequest
): Promise<void> {
  await requestEmpty("/register", {
    method: "POST",
    headers: {
      Accept: "application/json",
      "Content-Type": "application/json",
    },
    body: JSON.stringify(request),
  });
}

async function requestJson<T>(
  input: RequestInfo | URL,
  init?: RequestInit
): Promise<T> {
  const response = await fetch(input, init);
  if (!response.ok) throw await responseError(response);
  return (await response.json()) as T;
}

async function requestEmpty(
  input: RequestInfo | URL,
  init?: RequestInit
): Promise<void> {
  const response = await fetch(input, init);
  if (!response.ok) throw await responseError(response);
}

async function responseError(response: Response): Promise<PortalApiError> {
  const body = await response.text().catch(() => "");
  let message = body || `${response.status} ${response.statusText}`;
  let code: string | undefined;
  try {
    const parsed = JSON.parse(body) as {
      error?: { code?: string; message?: string };
      detail?: string;
    };
    message = parsed.error?.message ?? parsed.detail ?? message;
    code = parsed.error?.code;
  } catch {
    // Plain-text V1 compatibility errors are already useful as-is.
  }
  return new PortalApiError(response.status, message, code);
}

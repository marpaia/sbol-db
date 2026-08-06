import type { Capabilities } from "@/features/admin/backend/api";
import type { RecentJob } from "@/features/admin/jobs/api";
import {
  HttpError,
  parseStructuredErrorBody,
  requestJson,
} from "@/shared/api/http";

const ADMIN_API = "/api/v2/admin";

export class AdminApiError extends HttpError {
  constructor(status: number, message: string, code?: string) {
    super({ status, message, code });
  }
}

export interface AdminOverview {
  api: "v2-admin";
  policy: "authenticated_administrator";
  backend?: "postgres" | "sqlite" | "rocksdb";
  backend_name?: string;
  capabilities?: Capabilities;
  sections: Array<{ id: string; read: boolean; mutate: boolean }>;
  search: SearchStatus;
}

export interface AdminInstance {
  name: string;
  instance_url: string;
  uri_prefix: string;
  front_page_text: string;
  allow_public_signup: boolean;
  require_login: boolean;
  setup_required: boolean;
}

export type AdminInstancePatch = Partial<
  Pick<
    AdminInstance,
    | "name"
    | "instance_url"
    | "uri_prefix"
    | "front_page_text"
    | "allow_public_signup"
    | "require_login"
  >
>;

export interface AdminUser {
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

export interface AdminUsersResponse {
  total: number;
  limit: number;
  offset: number;
  items: AdminUser[];
}

export interface AdminUsersQuery {
  q?: string;
  limit?: number;
  offset?: number;
}

export interface CreateAdminUser {
  username: string;
  name: string;
  email: string;
  affiliation?: string;
  password: string;
  is_admin: boolean;
  is_curator: boolean;
  is_member: boolean;
}

export type UpdateAdminUser = Partial<
  Pick<
    AdminUser,
    "name" | "email" | "affiliation" | "is_admin" | "is_curator" | "is_member"
  >
>;

export interface RegistryIntegration {
  uri: string;
  url: string;
}

export interface AdminIntegrations {
  federation: { registered: boolean; url: string };
  registries: RegistryIntegration[];
  remotes: Record<string, Record<string, unknown>>;
  plugins: Record<string, Array<{ name: string; url: string }>>;
}

export interface SearchStatus {
  default_strategy: string;
  strategies: Array<{
    id: string;
    label?: string;
    description?: string;
    [key: string]: unknown;
  }>;
  recent_rebuilds?: RecentJob[];
}

export interface AdminAuditEvent {
  iri: string;
  action: string;
  actor: string;
  target: string;
  outcome: "attempted" | "succeeded" | "failed";
  detail: string | null;
  occurred_at: string;
}

export interface AdminAuditResponse {
  total: number;
  items: AdminAuditEvent[];
}

export interface CatalogCounts {
  resources: number;
  named_graphs: number;
  triples: number;
  sequences: number;
  ontologies: number;
}

export interface CatalogLiteral {
  value: string;
  datatype: string;
  language?: string;
}

export interface CatalogResourceMeta {
  display_id?: CatalogLiteral[];
  name?: CatalogLiteral[];
  description?: CatalogLiteral[];
  version?: CatalogLiteral[];
  types?: string[];
  sbol_types?: string[];
  roles?: string[];
  creators?: string[];
  top_level?: boolean;
}

export interface CatalogResource {
  iri: string;
  graph_count: number;
  meta: CatalogResourceMeta;
}

export interface CatalogResourceOccurrence {
  graph_iri: string;
  resource_iri: string;
  meta: CatalogResourceMeta;
}

export interface CatalogResourceDetail {
  resource: CatalogResource;
  occurrences: CatalogResourceOccurrence[];
}

export interface CatalogResourceLookup {
  found: CatalogResource[];
  missing: string[];
}

export interface CatalogGraph {
  id: string;
  iri: string;
  name: string | null;
  description: string | null;
  source_uri: string | null;
  serialization_format: string | null;
  triple_count: number | null;
  resource_count: number | null;
  created_at: string | null;
  updated_at: string | null;
}

export interface CatalogSequence {
  iri: string;
  graph_count: number;
  encoding_iri: string | null;
  elements: string | null;
  alphabet: string | null;
}

export interface CatalogTopClass {
  iri: string;
  count: number;
}

export interface CatalogOntology {
  prefix: string;
  name: string;
  source_url: string | null;
  version: string | null;
  term_count: number;
  imported_at: string;
}

export interface CatalogDashboard {
  counts: CatalogCounts;
  graphs: CatalogGraph[];
  top_classes: CatalogTopClass[];
  loaded_ontologies: CatalogOntology[];
}

export interface CursorPage<T> {
  items: T[];
  next_cursor?: string;
}

export interface CatalogPageQuery {
  after?: string;
  limit?: number;
  q?: string;
}

export interface CatalogResourceQuery extends CatalogPageQuery {
  class?: string;
  role?: string;
  graph?: string;
}

export interface CatalogTerm {
  type: "uri" | "bnode" | "literal";
  value: string;
  datatype?: string;
  language?: string;
}

export interface CatalogTriple {
  subject: CatalogTerm;
  predicate: CatalogTerm;
  object: CatalogTerm;
}

export type CompleteBackupTrigger = "manual" | "pre_deploy";

export interface CompleteBackupStatus {
  enabled: boolean;
  strategy: "complete_encrypted_checkpoint";
  components: Array<"rocksdb" | "blobs" | "search" | "acme">;
  recent: RecentJob[];
}

export interface CompleteBackupEnqueueResponse {
  job: RecentJob;
  deduplicated: boolean;
}

export interface EdgeSettings {
  version: number;
  hostname: string;
  acme_contact: string;
  acme_directory_url: string;
  http_redirect_enabled: boolean;
  tls_handshake_timeout_secs: number;
  backup_recovery_recipient: string;
  backup_repository_url: string;
  backup_interval_secs: number;
  backup_local_retention: number;
  minimum_free_bytes: number;
}

export type EdgeSettingsPatch = Partial<Omit<EdgeSettings, "version">>;

export interface EdgeAdminSnapshot {
  active: EdgeSettings;
  pending: EdgeSettings;
  restart_required: boolean;
  runtime: {
    profile: "production";
    layout_version: string;
    generation: string;
    data_dir: string;
  };
  health: {
    tls: {
      required: boolean;
      ready: boolean;
      certificate_not_after: string | null;
      certificate_expires_in_secs: number | null;
    };
    acme: {
      last_success_at: string | null;
      last_failure_at: string | null;
    };
    disk: {
      ready: boolean;
      available_bytes: number | null;
      minimum_free_bytes: number;
      error: string | null;
    } | null;
  };
  recovery: {
    activation_mode: "offline_cli";
    active_generation: string;
    previous_generation: string | null;
    last_operation: EdgeRecoveryEvent | null;
    history: EdgeRecoveryEvent[];
  };
}

export interface EdgeRecoveryEvent {
  status: "staged" | "activated" | "rollback_pending" | "rolled_back";
  backup_id: string;
  artifact_sha256: string;
  previous_generation: string | null;
  target_generation: string;
  updated_at: string;
}

export function fetchAdminOverview(signal?: AbortSignal) {
  return request<AdminOverview>("", { signal });
}

export function fetchCatalogDashboard(signal?: AbortSignal) {
  return request<CatalogDashboard>("/dashboard", { signal });
}

export function fetchCatalogGraphs(
  query: CatalogPageQuery = {},
  signal?: AbortSignal
) {
  return request<CursorPage<CatalogGraph>>(
    `/graphs${queryString(query)}`,
    { signal }
  );
}

export function fetchCatalogGraph(id: string, signal?: AbortSignal) {
  return request<CatalogGraph>(`/graphs/${encodeURIComponent(id)}`, { signal });
}

export function fetchCatalogGraphTriples(
  id: string,
  query: Pick<CatalogPageQuery, "after" | "limit"> = {},
  signal?: AbortSignal
) {
  return request<CursorPage<CatalogTriple>>(
    `/graphs/${encodeURIComponent(id)}/triples${queryString(query)}`,
    { signal }
  );
}

export function fetchCatalogResources(
  query: CatalogResourceQuery = {},
  signal?: AbortSignal
) {
  return request<CursorPage<CatalogResource>>(
    `/resources${queryString(query)}`,
    { signal }
  );
}

export function fetchCatalogResource(iri: string, signal?: AbortSignal) {
  return request<CatalogResourceDetail>(
    `/resources/lookup?iri=${encodeURIComponent(iri)}`,
    { signal }
  );
}

export function lookupCatalogResources(iris: string[]) {
  return request<CatalogResourceLookup>(
    "/resources/lookup",
    jsonRequest("POST", { iris })
  );
}

export function fetchCatalogSequences(
  query: CatalogPageQuery = {},
  signal?: AbortSignal
) {
  return request<CursorPage<CatalogSequence>>(
    `/sequences${queryString(query)}`,
    { signal }
  );
}

export function fetchAdminInstance(signal?: AbortSignal) {
  return request<AdminInstance>("/instance", { signal });
}

export function updateAdminInstance(payload: AdminInstancePatch) {
  return request<AdminInstance>("/instance", jsonRequest("PATCH", payload));
}

export function fetchAdminUsers(
  query: AdminUsersQuery = {},
  signal?: AbortSignal
) {
  const params = new URLSearchParams();
  if (query.q) params.set("q", query.q);
  if (typeof query.limit === "number") params.set("limit", String(query.limit));
  if (typeof query.offset === "number")
    params.set("offset", String(query.offset));
  const tail = params.toString();
  return request<AdminUsersResponse>(`/users${tail ? `?${tail}` : ""}`, {
    signal,
  });
}

export function createAdminUser(payload: CreateAdminUser) {
  return request<AdminUser>("/users", jsonRequest("POST", payload));
}

export function updateAdminUser(username: string, payload: UpdateAdminUser) {
  return request<AdminUser>(
    `/users/${encodeURIComponent(username)}`,
    jsonRequest("PATCH", payload)
  );
}

export function deleteAdminUser(username: string, confirmation: string) {
  return request<void>(
    `/users/${encodeURIComponent(username)}`,
    jsonRequest("DELETE", { confirmation })
  );
}

export function fetchAdminIntegrations(signal?: AbortSignal) {
  return request<AdminIntegrations>("/integrations", { signal });
}

export function joinFederation(administrator_email: string, url: string) {
  return request<{ status: string }>(
    "/federation",
    jsonRequest("POST", { administrator_email, url })
  );
}

export function syncFederation() {
  return request<{ status: string; count: number }>(
    "/federation/sync",
    jsonRequest("POST", {})
  );
}

export function saveRegistry(uri: string, url: string) {
  return request<{ status: string; uri: string }>(
    "/registries",
    jsonRequest("POST", { uri, url })
  );
}

export function deleteRegistry(uri: string, confirmation: string) {
  return request<void>(
    `/registries/${encodeURIComponent(uri)}`,
    jsonRequest("DELETE", { confirmation })
  );
}

export function saveRemote(remote: Record<string, unknown>) {
  return request<{ status: string; id: string }>(
    "/remotes",
    jsonRequest("POST", remote)
  );
}

export function deleteRemote(id: string, confirmation: string) {
  return request<void>(
    `/remotes/${encodeURIComponent(id)}`,
    jsonRequest("DELETE", { confirmation })
  );
}

export function savePlugin(payload: {
  category: string;
  id: string;
  name: string;
  url: string;
}) {
  return request<{ status: string; name: string }>(
    "/plugins",
    jsonRequest("POST", payload)
  );
}

export function deletePlugin(
  category: string,
  id: string,
  confirmation: string
) {
  return request<void>(
    `/plugins/${encodeURIComponent(category)}/${encodeURIComponent(id)}`,
    jsonRequest("DELETE", { confirmation })
  );
}

export function fetchSearchStatus(signal?: AbortSignal) {
  return request<SearchStatus>("/search", { signal });
}

export function rebuildSearch() {
  return request<{ job: RecentJob; deduplicated: boolean }>(
    "/search/rebuild",
    jsonRequest("POST", {})
  );
}

export function fetchAdminAudit(signal?: AbortSignal) {
  return request<AdminAuditResponse>("/audit?limit=200", { signal });
}

export function fetchCompleteBackupStatus(signal?: AbortSignal) {
  return request<CompleteBackupStatus>("/backup", { signal });
}

export function triggerCompleteBackup(
  trigger: CompleteBackupTrigger = "manual",
  idempotencyKey?: string
) {
  return request<CompleteBackupEnqueueResponse>(
    "/backup",
    jsonRequest("POST", {
      trigger,
      ...(idempotencyKey ? { idempotency_key: idempotencyKey } : {}),
    })
  );
}

export function fetchEdgeAdmin(signal?: AbortSignal) {
  return request<EdgeAdminSnapshot>("/edge", { signal });
}

export function updateEdgeAdmin(payload: EdgeSettingsPatch) {
  return request<EdgeAdminSnapshot>("/edge", jsonRequest("PATCH", payload));
}

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  return requestJson<T>(`${ADMIN_API}${path}`, init, responseError);
}

function jsonRequest(method: string, body: unknown): RequestInit {
  return {
    method,
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  };
}

function queryString(query: object) {
  const params = new URLSearchParams();
  for (const [key, value] of Object.entries(query)) {
    if (typeof value === "string" && value.length > 0) params.set(key, value);
    if (typeof value === "number") params.set(key, String(value));
  }
  const tail = params.toString();
  return tail ? `?${tail}` : "";
}

async function responseError(res: Response): Promise<AdminApiError> {
  const body = await res.text().catch(() => "");
  const value = parseStructuredErrorBody(body);
  return new AdminApiError(
    res.status,
    value?.message || `Request failed with HTTP ${res.status}`,
    value?.code
  );
}

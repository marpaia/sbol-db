/**
 * Network surface for the data lab bench.
 *
 * The SPARQL execute path posts to the canonical `/sparql` endpoint
 * directly. The SQL execute path (PR 3) lands under `/lab/api/sql/*`.
 * Both share a small `ApiError` envelope and uniform AbortSignal
 * support so the UI's Stop button cancels the in-flight request.
 */

export class ApiError extends Error {
  status: number;
  body: string;
  constructor(status: number, message: string, body: string) {
    super(message);
    this.status = status;
    this.body = body;
  }
}

async function asApiError(res: Response): Promise<ApiError> {
  const body = await res.text().catch(() => "");
  return new ApiError(
    res.status,
    `HTTP ${res.status}: ${res.statusText}`,
    body
  );
}

// ---------- SPARQL ----------

/** Outcome shape returned by `executeSparql`. */
export interface SparqlOutcome {
  /** Content-Type the server sent back. */
  contentType: string;
  /** Parsed JSON when the response is `application/sparql-results+json`,
   * otherwise the body as a string. */
  body: SparqlSelectResults | SparqlAskResult | string;
  /** Wall-clock time spent talking to the server, in ms. */
  elapsedMs: number;
  /** Whether the server set `X-SBOL-DB-Truncated`. */
  truncated: boolean;
}

export interface SparqlSelectResults {
  head: { vars: string[] };
  results: { bindings: Record<string, SparqlBinding>[] };
}

export interface SparqlAskResult {
  head: Record<string, never>;
  boolean: boolean;
}

export interface SparqlBinding {
  type: "uri" | "literal" | "bnode" | "typed-literal";
  value: string;
  datatype?: string;
  "xml:lang"?: string;
}

interface SparqlExecuteEnvelope {
  content_type: string;
  body: SparqlSelectResults | SparqlAskResult | string;
  elapsed_ms: number;
  truncated: boolean;
}

export async function executeSparql(
  query: string,
  signal?: AbortSignal
): Promise<SparqlOutcome> {
  const res = await fetch("/lab/api/sparql/execute", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ query }),
    signal,
  });
  if (!res.ok) throw await asApiError(res);
  const env = (await res.json()) as SparqlExecuteEnvelope;
  return {
    contentType: env.content_type,
    body: env.body,
    elapsedMs: env.elapsed_ms,
    truncated: env.truncated,
  };
}

/** True if the response is a SELECT-style result set. */
export function isSparqlSelect(
  b: SparqlOutcome["body"]
): b is SparqlSelectResults {
  return (
    typeof b === "object" &&
    b !== null &&
    "results" in b &&
    "bindings" in (b as SparqlSelectResults).results
  );
}

/** True if the response is an ASK boolean. */
export function isSparqlAsk(b: SparqlOutcome["body"]): b is SparqlAskResult {
  return typeof b === "object" && b !== null && "boolean" in b;
}

// ---------- SQL ----------

export interface SqlColumn {
  name: string;
  /** Postgres type name as reported by the server (`TEXT`, `INT4`, …). */
  pg_type: string;
}

export type SqlCell = string | number | boolean | null | unknown[] | object;

export interface SqlExecuteRequest {
  query: string;
  statement_timeout_ms?: number;
  row_limit?: number;
}

export interface SqlExecuteResponse {
  columns: SqlColumn[];
  rows: SqlCell[][];
  row_count: number;
  truncated: boolean;
  elapsed_ms: number;
  backend_pid: number;
}

export async function executeSql(
  req: SqlExecuteRequest,
  signal?: AbortSignal
): Promise<SqlExecuteResponse> {
  const res = await fetch("/lab/api/sql/execute", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(req),
    signal,
  });
  if (!res.ok) throw await asApiError(res);
  return (await res.json()) as SqlExecuteResponse;
}

// ---------- Validation (shared shape for both dialects) ----------

export interface ValidateError {
  message: string;
  line: number;
  column: number;
  end_line?: number | null;
  end_column?: number | null;
}

export interface ValidateResponse {
  ok: boolean;
  errors: ValidateError[];
}

export async function validateSql(
  query: string,
  signal?: AbortSignal
): Promise<ValidateResponse> {
  return validateImpl("/lab/api/sql/validate", query, signal);
}

export async function validateSparql(
  query: string,
  signal?: AbortSignal
): Promise<ValidateResponse> {
  return validateImpl("/lab/api/sparql/validate", query, signal);
}

async function validateImpl(
  path: string,
  query: string,
  signal?: AbortSignal
): Promise<ValidateResponse> {
  const res = await fetch(path, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ query }),
    signal,
  });
  if (!res.ok) throw await asApiError(res);
  return (await res.json()) as ValidateResponse;
}

// ---------- Schema introspection ----------

export interface SqlSchemaColumn {
  name: string;
  pg_type: string;
  nullable: boolean;
}

export interface SqlSchemaTable {
  name: string;
  columns: SqlSchemaColumn[];
}

export interface SqlSchema {
  tables: SqlSchemaTable[];
}

export interface SparqlSchemaPrefix {
  prefix: string;
  iri: string;
  from_ontology: boolean;
}

export interface SparqlSchemaClass {
  iri: string;
  count: number;
}

export interface SparqlSchema {
  prefixes: SparqlSchemaPrefix[];
  top_classes: SparqlSchemaClass[];
}

export async function fetchSqlSchema(signal?: AbortSignal): Promise<SqlSchema> {
  const res = await fetch("/lab/api/schema/sql", { signal });
  if (!res.ok) throw await asApiError(res);
  return (await res.json()) as SqlSchema;
}

export async function fetchSparqlSchema(
  signal?: AbortSignal
): Promise<SparqlSchema> {
  const res = await fetch("/lab/api/schema/sparql", { signal });
  if (!res.ok) throw await asApiError(res);
  return (await res.json()) as SparqlSchema;
}

// ---------- Dashboard overview ----------

export interface OverviewCounts {
  objects: number;
  documents: number;
  quads: number;
  sequences: number;
  validation_runs: number;
  ontologies: number;
}

export interface RecentDocument {
  id: string;
  name: string | null;
  source_uri: string | null;
  serialization_format: string;
  created_at: string;
  object_count: number;
}

export interface OverviewTopClass {
  iri: string;
  count: number;
}

export interface OverviewOntology {
  prefix: string;
  name: string;
  term_count: number;
}

export interface Overview {
  counts: OverviewCounts;
  recent_documents: RecentDocument[];
  top_classes: OverviewTopClass[];
  loaded_ontologies: OverviewOntology[];
}

export async function fetchOverview(signal?: AbortSignal): Promise<Overview> {
  const res = await fetch("/lab/api/overview", { signal });
  if (!res.ok) throw await asApiError(res);
  return (await res.json()) as Overview;
}

// ---------- Ontology loader ----------

export interface OntologyLoadRequest {
  prefix: string;
  url?: string;
  name?: string;
}

export interface OntologyLoadReport {
  prefix: string;
  source_url: string | null;
  version: string | null;
  term_count: number;
  closure_count: number;
  alias_count: number;
}

export async function loadOntology(
  req: OntologyLoadRequest,
  signal?: AbortSignal
): Promise<OntologyLoadReport> {
  const res = await fetch("/ontology", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(req),
    signal,
  });
  if (!res.ok) throw await asApiError(res);
  return (await res.json()) as OntologyLoadReport;
}

// ---------- Ontology browser ----------

export interface OntologyRecord {
  prefix: string;
  name: string;
  source_url?: string | null;
  version?: string | null;
  term_count: number;
  imported_at: string;
}

export interface OntologyTermRecord {
  iri: string;
  prefix: string;
  curie: string;
  name: string;
  definition?: string | null;
  is_obsolete: boolean;
  synonyms: string[];
}

export interface OntologyDescendant {
  iri: string;
  depth: number;
}

export async function listOntologies(
  signal?: AbortSignal
): Promise<OntologyRecord[]> {
  const res = await fetch("/ontology", { signal });
  if (!res.ok) throw await asApiError(res);
  return (await res.json()) as OntologyRecord[];
}

export async function fetchOntologyTerm(
  iri: string,
  signal?: AbortSignal
): Promise<OntologyTermRecord> {
  const res = await fetch(`/ontology/term?iri=${encodeURIComponent(iri)}`, {
    signal,
  });
  if (!res.ok) throw await asApiError(res);
  return (await res.json()) as OntologyTermRecord;
}

export async function fetchOntologyDescendants(
  iri: string,
  signal?: AbortSignal
): Promise<OntologyDescendant[]> {
  const res = await fetch(
    `/ontology/descendants?iri=${encodeURIComponent(iri)}`,
    { signal }
  );
  if (!res.ok) throw await asApiError(res);
  return (await res.json()) as OntologyDescendant[];
}

export interface OntologyTermsPage {
  prefix: string;
  total: number;
  limit: number;
  offset: number;
  terms: OntologyTermRecord[];
}

export interface OntologyTermsQuery {
  prefix: string;
  q?: string;
  limit?: number;
  offset?: number;
}

export async function listOntologyTerms(
  query: OntologyTermsQuery,
  signal?: AbortSignal
): Promise<OntologyTermsPage> {
  const parts: string[] = [`prefix=${encodeURIComponent(query.prefix)}`];
  if (query.q && query.q.length > 0) {
    parts.push(`q=${encodeURIComponent(query.q)}`);
  }
  if (typeof query.limit === "number") {
    parts.push(`limit=${query.limit}`);
  }
  if (typeof query.offset === "number") {
    parts.push(`offset=${query.offset}`);
  }
  const res = await fetch(`/ontology/terms?${parts.join("&")}`, { signal });
  if (!res.ok) throw await asApiError(res);
  return (await res.json()) as OntologyTermsPage;
}

// ---------- Observability ----------

export interface PoolStat {
  size: number;
  idle: number;
  in_use: number;
}

export interface PoolSnapshot {
  api: PoolStat;
  worker: PoolStat | null;
}

export interface BucketSnapshot {
  started_at: string;
  count: number;
  error_count: number;
  p50_ms: number;
  p95_ms: number;
  p99_ms: number;
  max_ms: number;
}

export interface RollingSnapshot {
  bucket_secs: number;
  window_buckets: number;
  buckets: BucketSnapshot[];
}

export interface ObservabilityHealth {
  ready: boolean;
  version: string;
  uptime_secs: number;
  snapshot_at: string;
}

export interface QueueDepthRow {
  status: "queued" | "running" | "succeeded" | "failed" | "cancelled" | "dead";
  queue: string;
  count: number;
}

export interface OldestQueuedAge {
  queue: string;
  age_secs: number;
}

export interface JobsSnapshot {
  queue_depth: QueueDepthRow[];
  oldest_age: OldestQueuedAge[];
  failures_24h: number;
}

export interface ObservabilitySummary {
  health: ObservabilityHealth;
  pool: PoolSnapshot;
  jobs: JobsSnapshot;
  rolling: RollingSnapshot;
}

export async function fetchObservabilitySummary(
  signal?: AbortSignal
): Promise<ObservabilitySummary> {
  const res = await fetch("/lab/api/observability/summary", { signal });
  if (!res.ok) throw await asApiError(res);
  return (await res.json()) as ObservabilitySummary;
}

export interface DatabaseSize {
  database: string;
  total_bytes: number;
}

export async function fetchPgDatabase(
  signal?: AbortSignal
): Promise<DatabaseSize> {
  const res = await fetch("/lab/api/observability/postgres/database", {
    signal,
  });
  if (!res.ok) throw await asApiError(res);
  return (await res.json()) as DatabaseSize;
}

export interface TableStats {
  name: string;
  rows_estimate: number;
  total_bytes: number;
  index_bytes: number;
  n_live_tup: number;
  n_dead_tup: number;
  last_vacuum: string | null;
  last_autovacuum: string | null;
  last_analyze: string | null;
}

export async function fetchPgTables(
  limit = 20,
  offset = 0,
  signal?: AbortSignal
): Promise<TableStats[]> {
  const res = await fetch(
    `/lab/api/observability/postgres/tables?limit=${limit}&offset=${offset}`,
    { signal }
  );
  if (!res.ok) throw await asApiError(res);
  return (await res.json()) as TableStats[];
}

export interface IndexStats {
  table: string;
  index: string;
  idx_scan: number;
  bytes: number;
}

export async function fetchPgIndexes(
  limit = 30,
  signal?: AbortSignal
): Promise<IndexStats[]> {
  const res = await fetch(
    `/lab/api/observability/postgres/indexes?limit=${limit}`,
    { signal }
  );
  if (!res.ok) throw await asApiError(res);
  return (await res.json()) as IndexStats[];
}

export interface PgActivity {
  pid: number;
  application_name: string | null;
  state: string | null;
  wait_event_type: string | null;
  wait_event: string | null;
  query: string | null;
  query_start: string | null;
  duration_secs: number | null;
  client_addr: string | null;
}

export async function fetchPgActivity(
  limit = 50,
  signal?: AbortSignal
): Promise<PgActivity[]> {
  const res = await fetch(
    `/lab/api/observability/postgres/activity?limit=${limit}`,
    { signal }
  );
  if (!res.ok) throw await asApiError(res);
  return (await res.json()) as PgActivity[];
}

export interface BlockingLock {
  blocker_pid: number;
  blocker_query: string | null;
  blocked_pid: number;
  blocked_query: string | null;
  mode: string | null;
  locktype: string | null;
}

export async function fetchPgLocks(
  signal?: AbortSignal
): Promise<BlockingLock[]> {
  const res = await fetch("/lab/api/observability/postgres/locks", { signal });
  if (!res.ok) throw await asApiError(res);
  return (await res.json()) as BlockingLock[];
}

export interface SlowQuery {
  queryid: string;
  query: string | null;
  calls: number;
  total_exec_ms: number;
  mean_exec_ms: number;
  rows: number;
}

export interface TableColumn {
  name: string;
  pg_type: string;
  nullable: boolean;
  default_expr: string | null;
  ordinal: number;
  comment: string | null;
  is_primary_key: boolean;
}

export interface OutgoingForeignKey {
  name: string;
  columns: string[];
  target_table: string;
  target_columns: string[];
}

export interface IncomingForeignKey {
  name: string;
  source_table: string;
  source_columns: string[];
  target_columns: string[];
}

export interface TableSchema {
  name: string;
  comment: string | null;
  columns: TableColumn[];
  foreign_keys_out: OutgoingForeignKey[];
  foreign_keys_in: IncomingForeignKey[];
}

export async function fetchPgTableSchema(
  name: string,
  signal?: AbortSignal
): Promise<TableSchema> {
  const res = await fetch(
    `/lab/api/observability/postgres/tables/${encodeURIComponent(name)}/schema`,
    { signal }
  );
  if (!res.ok) throw await asApiError(res);
  return (await res.json()) as TableSchema;
}

export type SlowQueriesResponse =
  | { status: "not_installed"; setup_hint: string }
  | { status: "installed"; rows: SlowQuery[] };

export async function fetchPgSlowQueries(
  limit = 20,
  signal?: AbortSignal
): Promise<SlowQueriesResponse> {
  const res = await fetch(
    `/lab/api/observability/postgres/slow-queries?limit=${limit}`,
    { signal }
  );
  if (!res.ok) throw await asApiError(res);
  return (await res.json()) as SlowQueriesResponse;
}

export type JobStatus =
  | "queued"
  | "running"
  | "succeeded"
  | "failed"
  | "cancelled"
  | "dead";

export interface RecentJob {
  id: string;
  kind: string;
  status: JobStatus;
  priority: number;
  queue: string;
  payload: unknown;
  result: unknown;
  error: string | null;
  idempotency_key: string | null;
  attempts: number;
  max_attempts: number;
  available_at: string;
  leased_by: string | null;
  lease_expires_at: string | null;
  parent_job_id: string | null;
  correlation_id: string | null;
  created_at: string;
  started_at: string | null;
  finished_at: string | null;
}

export interface RecentJobsQuery {
  limit?: number;
  queue?: string;
  status?: JobStatus;
}

export async function fetchRecentJobs(
  query: RecentJobsQuery = {},
  signal?: AbortSignal
): Promise<RecentJob[]> {
  const parts: string[] = [];
  if (typeof query.limit === "number") parts.push(`limit=${query.limit}`);
  if (query.queue) parts.push(`queue=${encodeURIComponent(query.queue)}`);
  if (query.status) parts.push(`status=${encodeURIComponent(query.status)}`);
  const qs = parts.length > 0 ? `?${parts.join("&")}` : "";
  const res = await fetch(`/lab/api/observability/jobs/recent${qs}`, {
    signal,
  });
  if (!res.ok) throw await asApiError(res);
  return (await res.json()) as RecentJob[];
}

export async function getJob(id: string, signal?: AbortSignal): Promise<RecentJob> {
  const res = await fetch(`/jobs/${encodeURIComponent(id)}`, { signal });
  if (!res.ok) throw await asApiError(res);
  return (await res.json()) as RecentJob;
}

export interface JobAttempt {
  id: number;
  job_id: string;
  attempt_no: number;
  worker_id: string;
  started_at: string;
  finished_at: string | null;
  status: JobStatus;
  error: string | null;
}

export async function fetchJobAttempts(
  id: string,
  signal?: AbortSignal
): Promise<JobAttempt[]> {
  const res = await fetch(`/jobs/${encodeURIComponent(id)}/attempts`, {
    signal,
  });
  if (!res.ok) throw await asApiError(res);
  return (await res.json()) as JobAttempt[];
}

export interface EnqueueJobRequest {
  kind: string;
  payload: unknown;
  queue?: string;
  priority?: number;
  max_attempts?: number;
  idempotency_key?: string;
  correlation_id?: string;
}

export interface EnqueueJobResult {
  job: RecentJob;
  deduplicated: boolean;
}

export async function enqueueJob(
  req: EnqueueJobRequest,
  signal?: AbortSignal
): Promise<EnqueueJobResult> {
  const res = await fetch(`/jobs`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(req),
    signal,
  });
  if (!res.ok) throw await asApiError(res);
  return (await res.json()) as EnqueueJobResult;
}

export interface CancelJobResponse {
  cancelled: boolean;
}

export async function cancelJob(
  id: string,
  signal?: AbortSignal
): Promise<CancelJobResponse> {
  const res = await fetch(`/jobs/${encodeURIComponent(id)}/cancel`, {
    method: "POST",
    signal,
  });
  if (!res.ok) throw await asApiError(res);
  return (await res.json()) as CancelJobResponse;
}

// ---------- Documents ----------

export interface DocumentSummary {
  id: string;
  document_iri: string | null;
  name: string | null;
  description: string | null;
  serialization_format: string;
  source_uri: string | null;
  created_by: string | null;
  created_at: string;
  object_count: number;
}

export interface DocumentsPage {
  total: number;
  limit: number;
  offset: number;
  documents: DocumentSummary[];
}

export interface DocumentsListQuery {
  limit?: number;
  offset?: number;
}

export async function listDocuments(
  query: DocumentsListQuery = {},
  signal?: AbortSignal
): Promise<DocumentsPage> {
  const qs = new URLSearchParams();
  if (typeof query.limit === "number") qs.set("limit", String(query.limit));
  if (typeof query.offset === "number") qs.set("offset", String(query.offset));
  const tail = qs.toString();
  const res = await fetch(`/lab/api/documents${tail ? `?${tail}` : ""}`, {
    signal,
  });
  if (!res.ok) throw await asApiError(res);
  return (await res.json()) as DocumentsPage;
}

export type SerializationFormat = "turtle" | "jsonld" | "rdfxml" | "ntriples";

export const SERIALIZATION_FORMATS: SerializationFormat[] = [
  "turtle",
  "jsonld",
  "rdfxml",
  "ntriples",
];

export function serializationLabel(format: SerializationFormat): string {
  switch (format) {
    case "turtle":
      return "Turtle";
    case "jsonld":
      return "JSON-LD";
    case "rdfxml":
      return "RDF/XML";
    case "ntriples":
      return "N-Triples";
  }
}

export function serializationContentType(format: SerializationFormat): string {
  switch (format) {
    case "turtle":
      return "text/turtle";
    case "jsonld":
      return "application/ld+json";
    case "rdfxml":
      return "application/rdf+xml";
    case "ntriples":
      return "application/n-triples";
  }
}

export interface ImportReport {
  document_id: string;
  object_count: number;
  quad_count: number;
  validation_status: "passed" | "failed";
  validation_issue_count: number;
}

export interface DocumentDetail {
  id: string;
  document_iri: string | null;
  name: string | null;
  description: string | null;
  serialization_format: string;
  source_uri: string | null;
  created_by: string | null;
  created_at: string;
  object_count: number;
  quad_count: number;
}

export interface ImportDocumentParams {
  format: SerializationFormat;
  body: string;
  name?: string;
  description?: string;
  source_uri?: string;
  document_iri?: string;
  created_by?: string;
}

export async function importDocument(
  params: ImportDocumentParams,
  signal?: AbortSignal
): Promise<ImportReport> {
  const qs = new URLSearchParams({ format: params.format });
  if (params.name) qs.set("name", params.name);
  if (params.description) qs.set("description", params.description);
  if (params.source_uri) qs.set("source_uri", params.source_uri);
  if (params.document_iri) qs.set("document_iri", params.document_iri);
  if (params.created_by) qs.set("created_by", params.created_by);
  const res = await fetch(`/documents?${qs.toString()}`, {
    method: "POST",
    headers: { "Content-Type": serializationContentType(params.format) },
    body: params.body,
    signal,
  });
  if (!res.ok) throw await asApiError(res);
  return (await res.json()) as ImportReport;
}

export async function getDocument(
  id: string,
  signal?: AbortSignal
): Promise<DocumentDetail> {
  const res = await fetch(`/lab/api/documents/${encodeURIComponent(id)}`, {
    signal,
  });
  if (!res.ok) throw await asApiError(res);
  return (await res.json()) as DocumentDetail;
}

export interface BulkImportDocument {
  format: SerializationFormat;
  body: string;
  name?: string;
  description?: string;
  source_uri?: string;
  document_iri?: string;
  created_by?: string;
}

export interface BulkImportResponse {
  imported: number;
  reports: ImportReport[];
}

export async function createDocumentsBulk(
  documents: BulkImportDocument[],
  signal?: AbortSignal
): Promise<BulkImportResponse> {
  const res = await fetch("/documents/bulk", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ documents }),
    signal,
  });
  if (!res.ok) throw await asApiError(res);
  return (await res.json()) as BulkImportResponse;
}

// ---------- Objects ----------

export interface SbolObjectRecord {
  id: string;
  iri: string;
  sbol_class?: string | null;
  display_id?: string | null;
  name?: string | null;
  persistent_identity?: string | null;
  version?: string | null;
  types?: string[] | null;
  roles?: string[] | null;
  data?: Record<string, unknown> | null;
  created_at?: string | null;
}

export interface ListObjectsQuery {
  sbol_class?: string;
  role?: string;
  document_id?: string;
  after?: string;
  limit?: number;
}

export interface ListObjectsResponse {
  objects: SbolObjectRecord[];
  next_cursor: string | null;
}

export async function listObjects(
  query: ListObjectsQuery = {},
  signal?: AbortSignal
): Promise<ListObjectsResponse> {
  const qs = new URLSearchParams();
  if (query.sbol_class) qs.set("sbol_class", query.sbol_class);
  if (query.role) qs.set("role", query.role);
  if (query.document_id) qs.set("document_id", query.document_id);
  if (query.after) qs.set("after", query.after);
  if (typeof query.limit === "number") qs.set("limit", String(query.limit));
  const tail = qs.toString();
  const res = await fetch(`/objects/list${tail ? `?${tail}` : ""}`, { signal });
  if (!res.ok) throw await asApiError(res);
  return (await res.json()) as ListObjectsResponse;
}

export async function getObjectByIri(
  iri: string,
  signal?: AbortSignal
): Promise<SbolObjectRecord> {
  const res = await fetch(`/objects?iri=${encodeURIComponent(iri)}`, {
    signal,
  });
  if (!res.ok) throw await asApiError(res);
  return (await res.json()) as SbolObjectRecord;
}

export interface LookupObjectsResponse {
  found: SbolObjectRecord[];
  missing: string[];
}

export async function lookupObjects(
  iris: string[],
  signal?: AbortSignal
): Promise<LookupObjectsResponse> {
  const res = await fetch("/objects/lookup", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ iris }),
    signal,
  });
  if (!res.ok) throw await asApiError(res);
  return (await res.json()) as LookupObjectsResponse;
}

export async function exportObjectRdf(
  id: string,
  format: SerializationFormat,
  signal?: AbortSignal
): Promise<string> {
  const res = await fetch(
    `/objects/${encodeURIComponent(id)}/rdf?format=${format}`,
    { signal }
  );
  if (!res.ok) throw await asApiError(res);
  return await res.text();
}

// ---------- Neighborhood ----------

export type NeighborhoodDirection = "forward" | "backward" | "both";

export interface NeighborhoodQuery {
  iri: string;
  depth?: number;
  direction?: NeighborhoodDirection;
  predicates?: string[];
  max_nodes?: number;
  literals?: boolean;
}

export interface NeighborhoodNode {
  id: string;
  depth: number;
  blank_node?: boolean;
  sbol_class?: string | null;
  display_id?: string | null;
  name?: string | null;
}

export type NeighborhoodObject =
  | { iri: string }
  | { blank: string }
  | { literal: string; datatype: string; language?: string };

export interface NeighborhoodEdge {
  subject: string;
  predicate: string;
  depth: number;
  object: NeighborhoodObject;
}

export interface NeighborhoodResult {
  root_iri: string;
  nodes: NeighborhoodNode[];
  edges: NeighborhoodEdge[];
  max_depth_reached: number;
  truncated: boolean;
}

function neighborhoodQueryString(q: NeighborhoodQuery): string {
  const qs = new URLSearchParams();
  qs.set("iri", q.iri);
  if (typeof q.depth === "number") qs.set("depth", String(q.depth));
  if (q.direction) qs.set("direction", q.direction);
  if (q.predicates && q.predicates.length > 0) {
    qs.set("predicates", q.predicates.join(","));
  }
  if (typeof q.max_nodes === "number") {
    qs.set("max_nodes", String(q.max_nodes));
  }
  if (typeof q.literals === "boolean") {
    qs.set("literals", String(q.literals));
  }
  return qs.toString();
}

export async function fetchNeighborhood(
  q: NeighborhoodQuery,
  signal?: AbortSignal
): Promise<NeighborhoodResult> {
  const res = await fetch(
    `/objects/neighborhood?${neighborhoodQueryString(q)}`,
    {
      signal,
    }
  );
  if (!res.ok) throw await asApiError(res);
  return (await res.json()) as NeighborhoodResult;
}

export async function fetchNeighborhoodRdf(
  q: NeighborhoodQuery,
  format: SerializationFormat,
  signal?: AbortSignal
): Promise<string> {
  const qs = `${neighborhoodQueryString(q)}&format=${format}`;
  const res = await fetch(`/objects/neighborhood.rdf?${qs}`, { signal });
  if (!res.ok) throw await asApiError(res);
  return await res.text();
}

// ---------- Sequences ----------

export type SequenceStrand = "+" | "-";

export interface SequenceMatch {
  sequence_iri: string;
  start: number;
  length: number;
  strand: SequenceStrand;
}

export interface SequenceSearchParams {
  pattern: string;
  max_hits?: number;
  forward_only?: boolean;
}

export async function sequenceSearch(
  params: SequenceSearchParams,
  signal?: AbortSignal
): Promise<SequenceMatch[]> {
  const qs = new URLSearchParams({ pattern: params.pattern });
  if (typeof params.max_hits === "number") {
    qs.set("max_hits", String(params.max_hits));
  }
  if (typeof params.forward_only === "boolean") {
    qs.set("forward_only", String(params.forward_only));
  }
  const res = await fetch(`/sequences/search?${qs.toString()}`, { signal });
  if (!res.ok) throw await asApiError(res);
  return (await res.json()) as SequenceMatch[];
}

export interface BatchSequenceMatch {
  pattern: string;
  matches: SequenceMatch[];
}

export interface SequenceSearchBatchRequest {
  patterns: string[];
  max_hits?: number;
  forward_only?: boolean;
}

export async function sequenceSearchBatch(
  req: SequenceSearchBatchRequest,
  signal?: AbortSignal
): Promise<BatchSequenceMatch[]> {
  const res = await fetch("/sequences/search", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(req),
    signal,
  });
  if (!res.ok) throw await asApiError(res);
  return (await res.json()) as BatchSequenceMatch[];
}

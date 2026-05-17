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

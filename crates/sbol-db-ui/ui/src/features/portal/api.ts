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
    profile_management: boolean;
    password_change: boolean;
    password_reset: boolean;
    collaboration: boolean;
    data_lab: boolean;
    sql_console: boolean;
  };
  machine_access?: {
    api_url: string;
    mcp_url?: string;
    authorization_issuer?: string;
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
  roles: string[];
  owners: string[];
  created_at: string | null;
  modified_at: string | null;
  score: number;
}

export interface PortalSearchResponse {
  items: PortalSearchHit[];
  total: number;
  offset: number;
  limit: number;
  sort: DiscoverySort;
  direction: SortDirection;
}

export type DiscoverySort =
  | "relevance"
  | "name"
  | "created"
  | "modified"
  | "iri";

export type SortDirection = "asc" | "desc";

export interface PortalSearchQuery {
  q?: string;
  type?: string;
  role?: string;
  collection?: string;
  owner?: string;
  provenance?: string;
  createdAfter?: string;
  createdBefore?: string;
  modifiedAfter?: string;
  modifiedBefore?: string;
  sort?: DiscoverySort;
  direction?: SortDirection;
  offset?: number;
  limit?: number;
}

export interface DiscoveryFacetValue {
  iri: string;
  label: string;
  curie: string | null;
  count: number;
}

export interface DiscoveryFacets {
  types: DiscoveryFacetValue[];
  roles: DiscoveryFacetValue[];
}

export interface PortalObjectSummary {
  uri: string;
  display_id: string | null;
  name: string | null;
  description: string | null;
  object_type: string | null;
}

export interface SequenceSearchHit extends PortalObjectSummary {
  percent_match: number;
  strand: string;
  cigar: string;
}

export interface SequenceSearchResponse {
  items: SequenceSearchHit[];
  total: number;
}

export interface SimilarHit extends PortalObjectSummary {
  pagerank: number;
}

export interface SimilarResponse {
  items: SimilarHit[];
  total: number;
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

export type ObjectContentState =
  | "available"
  | "empty"
  | "partial"
  | "unsupported";

export interface ObjectReference {
  uri: string;
  display_id: string | null;
  name: string | null;
  description: string | null;
  object_type: string | null;
}

export interface ObjectReferenceSection {
  state: ObjectContentState;
  items: ObjectReference[];
  note: string | null;
}

export type ObjectPropertyValue =
  | { kind: "resource"; value: string }
  | { kind: "blank_node"; value: string }
  | {
      kind: "literal";
      value: string;
      datatype: string;
      language: string | null;
    };

export interface ObjectProperty {
  predicate: string;
  values: ObjectPropertyValue[];
}

export interface ObjectAttachment {
  uri: string;
  name: string | null;
  hash: string | null;
  size: number | null;
  format: string | null;
  source: string | null;
  resolved: boolean;
}

export type ObjectVisualGlyph =
  | "promoter"
  | "coding_sequence"
  | "ribosome_entry_site"
  | "terminator"
  | "operator"
  | "origin_of_replication"
  | "unspecified";

export interface ObjectVisualFeature {
  uri: string;
  label: string;
  roles: string[];
  glyph: ObjectVisualGlyph;
  start: number | null;
  end: number | null;
  orientation: string | null;
}

export interface PortalObjectDetails {
  iri: string;
  persistent_identity: string | null;
  display_id: string | null;
  version: string | null;
  name: string | null;
  description: string | null;
  object_type: string;
  types: string[];
  roles: string[];
  source_graph: string | null;
  visibility: "public" | "restricted";
  owners: string[];
  created_at: string | null;
  modified_at: string | null;
  provenance: {
    creators: string[];
    derived_from: string[];
    generated_by: string[];
    mutable_source: string[];
    citations: string[];
  };
  sequence_content: {
    state: ObjectContentState;
    elements: string | null;
    encoding: string | null;
    length: number | null;
    note: string | null;
  };
  sequences: ObjectReferenceSection;
  features: ObjectReferenceSection;
  visualization: {
    state: ObjectContentState;
    sequence_length: number | null;
    features: ObjectVisualFeature[];
    note: string | null;
  };
  interactions: ObjectReferenceSection;
  collections: ObjectReferenceSection;
  members: ObjectReferenceSection;
  attachments: {
    state: ObjectContentState;
    items: ObjectAttachment[];
    note: string | null;
  };
  uses: ObjectReferenceSection;
  twins: ObjectReferenceSection;
  properties: ObjectProperty[];
  content_fingerprint: string | null;
}

export type ContributionFormat =
  | "rdfxml"
  | "turtle"
  | "jsonld"
  | "ntriples"
  | "genbank"
  | "fasta";

export type ContributionOverwrite = "fail" | "replace" | "merge";

export interface ContributionRequest {
  id: string;
  version: string;
  name?: string;
  description?: string;
  citations: string[];
  creator_name?: string;
  format: ContributionFormat;
  overwrite: ContributionOverwrite;
  content: string;
}

export interface ContributionPreview {
  valid: true;
  source_format: string;
  source_standard: "sbol2" | "sbol3" | "rdf" | "genbank" | "fasta";
  normalized_standard: "sbol2" | "sbol3" | "rdf";
  collection_uri: string;
  persistent_identity: string;
  graph: string;
  members: string[];
  triple_count: number;
  collision: boolean;
  consequence: "create" | "reject_conflict" | "replace" | "merge";
  notices: Array<{
    code: string;
    level: "info" | "warning";
    message: string;
  }>;
}

export interface ContributionCreated {
  collection_uri: string;
  persistent_identity: string;
  members: string[];
  graph: string;
  triple_count: number;
}

export interface ObjectEditRequest {
  name?: string;
  description?: string;
  citations?: string[];
}

export interface PublishRequest {
  id: string;
  version: string;
  name?: string;
  description?: string;
  citations: string[];
  overwrite: ContributionOverwrite;
}

export interface PublishResult {
  collection_uri: string;
  members: string[];
  triple_count: number;
}

export type AccountProfile = SessionUser;

export interface SharedObjectsResponse {
  items: PortalObjectDetails[];
  total: number;
}

export interface Collaborator {
  username: string;
  name: string;
  graph_uri: string;
  is_curator: boolean;
}

export interface CollaboratorsResponse {
  owners: Collaborator[];
  viewers: Collaborator[];
}

export type AuditAction =
  | "share_granted"
  | "share_revoked"
  | "ownership_transferred"
  | "review_requested"
  | "review_approved"
  | "review_changes_requested";

export interface AuditEvent {
  iri: string;
  object_iri: string;
  action: AuditAction;
  actor_graph: string;
  subject_graph: string | null;
  note: string | null;
  occurred_at: string;
}

export type ReviewStatus = "pending" | "approved" | "changes_requested";

export interface ReviewCase {
  object_iri: string;
  curator_graph: string;
  requested_by_graph: string;
  status: ReviewStatus;
  updated_at: string;
  note: string | null;
  events: AuditEvent[];
}

export interface ReviewListResponse {
  items: ReviewCase[];
  total: number;
}

export interface ActivityResponse {
  items: AuditEvent[];
  total: number;
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
  query: PortalSearchQuery,
  signal?: AbortSignal
): Promise<PortalSearchResponse> {
  const params = new URLSearchParams();
  if (query.q) params.set("q", query.q);
  if (query.type) params.set("type", query.type);
  if (query.role) params.set("role", query.role);
  if (query.collection) params.set("collection", query.collection);
  if (query.owner) params.set("owner", query.owner);
  if (query.provenance) params.set("provenance", query.provenance);
  if (query.createdAfter) params.set("created_after", query.createdAfter);
  if (query.createdBefore) params.set("created_before", query.createdBefore);
  if (query.modifiedAfter) params.set("modified_after", query.modifiedAfter);
  if (query.modifiedBefore) params.set("modified_before", query.modifiedBefore);
  if (query.sort) params.set("sort", query.sort);
  if (query.direction) params.set("direction", query.direction);
  if (query.offset !== undefined) params.set("offset", String(query.offset));
  if (query.limit !== undefined) params.set("limit", String(query.limit));
  return requestJson<PortalSearchResponse>(
    `/api/v2/search${params.size > 0 ? `?${params}` : ""}`,
    { signal }
  );
}

export async function fetchDiscoveryFacets(
  signal?: AbortSignal
): Promise<DiscoveryFacets> {
  return requestJson<DiscoveryFacets>("/api/v2/search/facets", { signal });
}

export async function searchPortalSequences(
  query: { q: string; mode: "global" | "exact"; limit: number },
  signal?: AbortSignal
): Promise<SequenceSearchResponse> {
  const params = new URLSearchParams({
    q: query.q,
    mode: query.mode,
    limit: String(query.limit),
  });
  return requestJson<SequenceSearchResponse>(
    `/api/v2/sequences/search?${params}`,
    { signal }
  );
}

export async function fetchSimilarObjects(
  iri: string,
  signal?: AbortSignal
): Promise<SimilarResponse> {
  return requestJson<SimilarResponse>(
    `/api/v2/objects/${encodeURIComponent(iri)}/similar`,
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

export async function fetchPortalObjectDetails(
  iri: string,
  signal?: AbortSignal
): Promise<PortalObjectDetails> {
  return requestJson<PortalObjectDetails>(
    `/api/v2/objects/${encodeURIComponent(iri)}/details`,
    { signal }
  );
}

export type PortalDownloadFormat =
  | "sbol"
  | "sbolnr"
  | "fasta"
  | "gb"
  | "gff"
  | "omex";

export async function downloadPortalObject(
  iri: string,
  format: PortalDownloadFormat,
  version?: "sbol2" | "sbol3"
): Promise<void> {
  const query = new URLSearchParams({ format });
  if (version) query.set("version", version);
  const response = await fetch(
    `/api/v2/objects/${encodeURIComponent(iri)}?${query}`
  );
  if (!response.ok) throw await responseError(response);

  const blob = await response.blob();
  if (blob.size === 0) {
    throw new PortalApiError(
      422,
      "The server returned an empty download for this format.",
      "empty_download"
    );
  }

  const disposition = response.headers.get("content-disposition") ?? "";
  const filename =
    disposition.match(/filename="([^"]+)"/)?.[1] ??
    `sbol-db-object.${downloadExtension(format)}`;
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = filename;
  document.body.append(anchor);
  anchor.click();
  anchor.remove();
  window.setTimeout(() => URL.revokeObjectURL(url), 0);
}

function downloadExtension(format: PortalDownloadFormat): string {
  switch (format) {
    case "sbol":
    case "sbolnr":
      return "xml";
    case "fasta":
      return "fasta";
    case "gb":
      return "gb";
    case "gff":
      return "gff";
    case "omex":
      return "omex";
  }
}

export async function fetchAccount(
  signal?: AbortSignal
): Promise<AccountProfile> {
  return requestJson<AccountProfile>("/api/v2/account", { signal });
}

export async function updateAccount(request: {
  name?: string;
  affiliation?: string;
}): Promise<AccountProfile> {
  return requestJson<AccountProfile>("/api/v2/account", {
    method: "PATCH",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(request),
  });
}

export async function changeAccountPassword(request: {
  current_password: string;
  new_password: string;
}): Promise<void> {
  await requestEmpty("/api/v2/account/password", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(request),
  });
}

export async function fetchSharedObjects(
  signal?: AbortSignal
): Promise<SharedObjectsResponse> {
  return requestJson<SharedObjectsResponse>("/api/v2/account/shared", {
    signal,
  });
}

export async function fetchCollaborators(
  iri: string,
  signal?: AbortSignal
): Promise<CollaboratorsResponse> {
  return requestJson<CollaboratorsResponse>(
    `/api/v2/objects/${encodeURIComponent(iri)}/shares`,
    { signal }
  );
}

export async function grantObjectShare(
  iri: string,
  user: string
): Promise<void> {
  await requestEmpty(`/api/v2/objects/${encodeURIComponent(iri)}/shares`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ user }),
  });
}

export async function revokeObjectShare(
  iri: string,
  user: string
): Promise<void> {
  await requestEmpty(
    `/api/v2/objects/${encodeURIComponent(iri)}/shares/${encodeURIComponent(user)}`,
    { method: "DELETE" }
  );
}

export async function transferObjectOwnership(
  iri: string,
  user: string
): Promise<void> {
  await requestEmpty(`/api/v2/objects/${encodeURIComponent(iri)}/owner`, {
    method: "PUT",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ user }),
  });
}

export async function fetchReviews(
  signal?: AbortSignal
): Promise<ReviewListResponse> {
  return requestJson<ReviewListResponse>("/api/v2/reviews", { signal });
}

export async function fetchObjectReview(
  iri: string,
  signal?: AbortSignal
): Promise<ReviewCase | null> {
  const response = await fetch(
    `/api/v2/objects/${encodeURIComponent(iri)}/reviews`,
    { signal }
  );
  if (response.status === 404) return null;
  if (!response.ok) throw await responseError(response);
  return (await response.json()) as ReviewCase;
}

export async function requestObjectReview(
  iri: string,
  curator: string,
  note?: string
): Promise<ReviewCase> {
  return requestJson<ReviewCase>(
    `/api/v2/objects/${encodeURIComponent(iri)}/reviews`,
    {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ curator, note: note || undefined }),
    }
  );
}

export async function decideObjectReview(
  iri: string,
  decision: "approve" | "request_changes",
  note?: string
): Promise<ReviewCase> {
  return requestJson<ReviewCase>(
    `/api/v2/objects/${encodeURIComponent(iri)}/reviews/decision`,
    {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ decision, note: note || undefined }),
    }
  );
}

export async function fetchObjectActivity(
  iri: string,
  signal?: AbortSignal
): Promise<ActivityResponse> {
  return requestJson<ActivityResponse>(
    `/api/v2/objects/${encodeURIComponent(iri)}/activity`,
    { signal }
  );
}

export async function validateContribution(
  request: ContributionRequest
): Promise<ContributionPreview> {
  return requestJson<ContributionPreview>("/api/v2/collections/validate", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(request),
  });
}

export async function createContribution(
  request: ContributionRequest
): Promise<ContributionCreated> {
  return requestJson<ContributionCreated>("/api/v2/collections", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(request),
  });
}

export async function editPortalObject(
  iri: string,
  request: ObjectEditRequest
): Promise<void> {
  await requestJson<unknown>(`/api/v2/objects/${encodeURIComponent(iri)}`, {
    method: "PATCH",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(request),
  });
}

export async function addCollectionMember(
  collection: string,
  member: string
): Promise<void> {
  await requestEmpty(
    `/api/v2/collections/${encodeURIComponent(collection)}/members`,
    {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ member }),
    }
  );
}

export async function removeCollectionMember(
  collection: string,
  member: string
): Promise<void> {
  await requestEmpty(
    `/api/v2/collections/${encodeURIComponent(collection)}/members/${encodeURIComponent(member)}`,
    { method: "DELETE" }
  );
}

export async function publishPortalObject(
  iri: string,
  request: PublishRequest
): Promise<PublishResult> {
  return requestJson<PublishResult>(
    `/api/v2/objects/${encodeURIComponent(iri)}/publish`,
    {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(request),
    }
  );
}

export async function deletePortalObject(iri: string): Promise<void> {
  await requestEmpty(`/api/v2/objects/${encodeURIComponent(iri)}`, {
    method: "DELETE",
  });
}

export async function deleteCollection(iri: string): Promise<void> {
  await requestEmpty(`/api/v2/collections/${encodeURIComponent(iri)}`, {
    method: "DELETE",
  });
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

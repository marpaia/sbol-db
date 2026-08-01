# CLI, SBOL Identity, and agent access

SBOL DB exposes one permission model through three machine-facing surfaces:

- the V2 HTTP API owns registry state and submission semantics;
- the `sbol` CLI in the
  [`sbol-rs` repository](https://github.com/SynBioDex/sbol-rs/tree/master/crates/sbol-cli)
  owns local SBOL file workflows and calls that API; and
- the MCP endpoint gives agents a scoped view of the same application services.

SBOL Identity is the authorization layer shared by those surfaces and by other
ecosystem applications. Registry passwords remain inside the SBOL DB sign-in
page; public clients receive short-lived, scoped credentials through browser
authorization code with S256 PKCE.

This separation is intentional. The CLI remains part of the SBOL SDK rather
than becoming a second SBOL DB binary, while registry authorization, ACLs,
identity minting, collision handling, review state, and provenance stay
server-authoritative.

## Runtime discovery

`GET /api/v2/instance` includes a `machine_access` object when the server knows
its canonical origin:

```json
{
  "machine_access": {
    "api_url": "http://127.0.0.1:8888/api/v2",
    "mcp_url": "http://127.0.0.1:8888/mcp",
    "authorization_issuer": "http://127.0.0.1:8888"
  }
}
```

The server derives the origin from its bound listener during local development,
so a server on an ephemeral port advertises that port. Use that advertised
loopback address while testing; `https://sbol.io` is the intended production
origin, not a requirement for local development.

A deployment behind a reverse proxy must supply its externally reachable
origin:

```sh
SBOL_DB_PUBLIC_ORIGIN=https://sbol.io sbol-db server
```

The value must be an HTTP or HTTPS origin with no path, credentials, query, or
fragment. Set `SBOL_DB_MCP_ENABLED=false` to leave `/mcp` unmounted and omit its
URL from instance discovery. SBOL Identity and V2 API discovery remain
available independently.

New accounts and designs use the resolved public origin as their identity
prefix, so an ephemeral local server mints beneath its exact
`http://127.0.0.1:<port>/` origin and the hosted registry mints beneath
`https://sbol.io/`. A migrated registry that must preserve an existing
SynBioHub `databasePrefix` sets `SBOL_DB_DATABASE_PREFIX` explicitly. This
identifier prefix is independent of the OAuth/API transport origin and does
not change already stored graph identities.

## SBOL Identity

SBOL DB acts as an OAuth authorization server and an OpenID Connect provider.
It uses the existing browser session and account database as the resource-owner
authentication step, then shows a dedicated consent screen naming the client
and each requested capability. API, MCP, and UserInfo bearer tokens never stand
in for that interactive browser session, so one delegated grant cannot approve
a broader grant.

### Discovery and endpoints

| Endpoint | Purpose |
|---|---|
| `/.well-known/oauth-authorization-server` | OAuth authorization-server metadata |
| `/.well-known/openid-configuration` | OpenID Connect provider metadata |
| `/.well-known/oauth-protected-resource/api/v2` | V2 API resource metadata |
| `/.well-known/oauth-protected-resource/mcp` | MCP resource metadata |
| `/oauth/register` | Dynamic registration for public clients |
| `/oauth/authorize` | Browser sign-in and consent |
| `/oauth/token` | Authorization-code exchange and refresh rotation |
| `/oauth/revoke` | Access- or refresh-token revocation |
| `/oauth/jwks` | Public Ed25519 verification key |
| `/oauth/userinfo` | OpenID Connect UserInfo claims |

Public clients have no client secret. Registered callbacks must use HTTPS, or
HTTP on `localhost`, `127.0.0.1`, or `::1`. Authorization codes expire after
five minutes, require an exact redirect URI and S256 verifier, and are consumed
once. Access tokens expire after one hour. Refresh tokens expire after 30 days
and rotate on use. Discovery, registration, token, revocation, JWKS, and
UserInfo responses include CORS headers for public browser clients; the
authorization endpoint still uses top-level browser navigation and the normal
same-origin session.

Only credential hashes are stored in PostgreSQL, SQLite, or RocksDB. Plain
authorization codes and tokens exist only in the response that issues them and
in the client that presents them.

### Resources and scopes

Every access token is bound to one exact audience:

| Resource | Intended client | Accepted scopes |
|---|---|---|
| `<origin>/api/v2` | `sbol` CLI and typed API clients | `sbol:read`, `sbol:write`, `sbol:share`, `sbol:review` |
| `<origin>/mcp` | MCP-capable agents | `sbol:read`, `sbol:write`, `sbol:share`, `sbol:review` |
| `<origin>/oauth/userinfo` | “Sign in with SBOL” clients | `openid`, `profile`, `email` |

An API token cannot be replayed at MCP, and an identity token cannot be used as
a registry credential. For the V2 API, delegated authorization is enforced in
addition to the account's normal ownership, sharing, membership, and curator
checks:

- ordinary reads and submission preview require `sbol:read`;
- design mutations require `sbol:read sbol:write`;
- sharing and ownership operations require `sbol:read sbol:share`;
- review and activity operations require `sbol:read sbol:review`; and
- delegated OAuth clients cannot access administrator routes or mutate account
  credentials.

Compatibility API tokens and same-origin browser sessions remain first-party
credentials. They do not become valid MCP credentials.

### Sign in with SBOL

An ecosystem application such as Flapjack or SBOL Canvas can discover
`/.well-known/openid-configuration`, dynamically register its callback, and
request `openid profile email`. The authorization-code response produces an
EdDSA-signed ID token. Its public, stable subject is the SBOL account UUID; the
optional profile claims are:

- `preferred_username`, `name`, and `affiliation` under `profile`; and
- `email` under `email`.

Administrative, curator, and membership flags are not identity claims. An app
that needs registry capabilities requests a resource-bound API grant and the
corresponding SBOL scopes instead of inferring authorization from an ID token.

ID tokens carry issuer, audience, issued-at, expiry, and the client's nonce.
Clients verify them with `/oauth/jwks` and must check issuer, audience, expiry,
and nonce.

The typed Python package includes `SbolIdentityClient` for provider discovery,
public-client registration, PKCE authorization requests, callback-state
validation, code exchange, refresh rotation, revocation, and UserInfo. Browser
routing and durable token storage remain explicit responsibilities of the host
application.

## `sbol registry`

The `sbol` binary uses the V2 API through the typed
`sbol-registry-client` crate:

```sh
sbol registry login
sbol registry logout
sbol registry status
sbol registry pull https://sbol.io/public/igem/BBa_J23100/1 -o design.ttl
sbol registry push design.ttl --preview
sbol registry push design.ttl
```

`sbol registry login` selects `https://sbol.io` by default. A university or
local instance is explicit:

```sh
sbol registry login https://sbol.my-university.edu
sbol registry login http://127.0.0.1:8888
```

When the instance advertises SBOL Identity, login opens its authorization page
in the browser and listens on a temporary loopback callback. The CLI requests
`sbol:read sbol:write` for the advertised V2 API resource, persists the access
and refresh context in its mode-`0600` profile, and refreshes before a command
when fewer than 30 seconds remain. `sbol registry logout` revokes both the
access and refresh credentials before removing the local profile; if the
registry is unreachable, it still removes the local copy and reports that
remote revocation could not be confirmed.

An older instance without an authorization issuer uses the compatibility
username/password exchange. `--identifier` or `--password-stdin` explicitly
selects that compatibility path. Controlled automation can provide an already
scoped token as `SBOL_ACCESS_TOKEN` without writing a profile.

Initial push is collision-safe by default. The client always calls
`POST /api/v2/collections/validate` first; the server parses and validates the
content, computes future identities, and reports whether the operation would
create, replace, merge, or conflict. `--dry-run` stops there without a write
(`--preview` remains a compatibility alias).
Commit calls `POST /api/v2/collections` with the same request. The default
collision policy is `fail`; `replace` and `merge` must be chosen explicitly.

`sbol init` creates `sbol.toml` and `designs/`, but deliberately not
`sbol.lock`. The first successful tracked pull or push creates the lock with a
local file hash and the server's representation-independent biological-content
ETag. `sbol status` and `sbol sync` then distinguish clean, local-only,
remote-only, and simultaneous changes. Tracked updates call
`PUT /api/v2/collections/{iri}/content` with the exact lock ETag in `If-Match`;
the server atomically rejects a stale write. Synchronization never infers
deletion or RDF merge. Both TOML files are credential-free and intended for
version control.

## MCP endpoint

An MCP-capable client connects to the advertised `<origin>/mcp` URL. On a
`401`, it follows the `resource_metadata` link, discovers the SBOL Identity
issuer, performs browser authorization with PKCE, and retries with an access
token issued for that exact MCP resource. Write, sharing, and review calls use
incremental `insufficient_scope` challenges rather than asking every client for
all permissions at connection time.

`POST /mcp` implements the MCP
[Streamable HTTP transport](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports)
without server-side sessions. Each request is one JSON-RPC message and
requires:

- `Content-Type: application/json`;
- an `Accept` header covering `application/json` and `text/event-stream`; and
- an SBOL Identity bearer token for the exact MCP audience.

The server supports protocol versions `2025-11-25`, `2025-06-18`, and
`2025-03-26`. It does not allocate MCP session IDs or open unsolicited SSE
streams; `GET /mcp` returns `405 Method Not Allowed` with `POST` in `Allow`.
Browser-origin requests are accepted only from the configured SBOL DB origin.

### Agent capabilities

| Tool | Scope | Capability |
|---|---|---|
| `search_designs` | read | Search visible designs by text, type, or biological role. |
| `get_design` | read | Open a normalized design record with biological and provenance context. |
| `download_design` | read | Render SBOL 2/3, GenBank, FASTA, GFF3, or OMEX. |
| `get_collection_sync_state` | read | Read complete biological collection content and its strong content ETag. |
| `search_sequences` | read | Align a nucleotide query against visible sequences. |
| `find_similar_designs` | read | Find visible cluster-related designs. |
| `validate_design_upload` | read + write | Validate create-only content and return a prepared change without writing designs. |
| `upload_design_collection` | read + write | Compatibility commit tool for a prepared upload token. |
| `prepare_design_metadata_update` | read + write | Prepare owned metadata, notes, source, or citation changes. |
| `prepare_design_publication` | read + write | Prepare publication under an explicit public identity and collision policy. |
| `prepare_collection_update` | read + write | Validate a complete collection replacement against its current ETag. |
| `list_design_collaborators` | read + share | Inspect owners and read-only collaborators. |
| `prepare_design_sharing` | read + share | Resolve and prepare a grant or revocation of read access. |
| `list_reviews` | read + review | Open the signed-in account's review queue. |
| `prepare_design_review` | read + review | Prepare assignment of an active curator to an owned design. |
| `prepare_review_decision` | read + review | Prepare approval or requested changes as a curator. |
| `apply_prepared_change` | scopes captured by plan | Consume and apply one reviewed, principal-bound change. |
| `get_design_activity` | read + review | Trace ownership, sharing, edit, publication, and review activity. |

Every tool calls the same application facade as the V2 and compatibility APIs.
Public, shared, and private visibility is computed from the authenticated
account's graph scope. A private design is returned to its owner or an
authorized collaborator and appears missing to an unrelated account.

Agent mutations use durable prepared changes instead of a boolean confirmation
flag. A preparation call validates and authorizes the exact payload, stores
only the hash of an opaque one-time token, and returns the intended effect,
input hash, expiry, and token for the agent to show to the user. The stored
change is bound to the SBOL account, OAuth client, MCP audience, and exact
scopes that prepared it. `apply_prepared_change` consumes that token once and
commits only the stored payload; a caller cannot substitute different input at
apply time.

Metadata and publication preparations capture the visible design snapshot.
Complete collection replacements capture the current biological-content ETag.
Apply fails without changing registry data if either baseline has become stale.
Initial creation and publication continue to default to collision policy
`fail`. The compatibility `upload_design_collection` tool accepts the same
durable upload token, but new clients should use the generic apply tool.

The server also advertises typed `sbol://` resources for the connected
registry, signed-in account, review queue, normalized designs, and serialized
design content. Resource reads and tool calls use the same visibility rules:
private designs appear missing unless the signed-in account owns them or has
been granted access.

Downloads are returned inline up to 8 MiB. Text formats use UTF-8 and OMEX uses
base64; larger artifacts direct the caller to the authenticated REST download.

## Production configuration

OpenID Connect clients need the same public verification key before and after a
restart and across replicas. Production deployments must set
`SBOL_DB_IDENTITY_SIGNING_KEY` to a stable base64-encoded Ed25519 PKCS#8 private
key. For example, generate the secret material outside the application and
install it through the deployment's secret manager:

```sh
openssl genpkey -algorithm ED25519 -outform DER | base64
```

The default key is process-local for tests and loopback development. A
non-loopback server without a configured key logs a warning because ID tokens
issued before a restart will no longer verify afterward.

Production identity deployments should also:

- serve the public origin over HTTPS;
- set `SBOL_DB_SESSION_COOKIE_SECURE=true`;
- keep the signing key out of manifests, logs, and command history;
- share the same signing key across replicas; and
- rate-limit dynamic registration and authorization endpoints at the ingress.

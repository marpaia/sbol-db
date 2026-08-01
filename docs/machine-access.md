# CLI and agent access

SBOL DB exposes one permission model through three machine-facing surfaces:

- the V2 HTTP API owns registry state and submission semantics;
- the `sbol` CLI in the
  [`sbol-rs` repository](https://github.com/SynBioDex/sbol-rs/tree/master/crates/sbol-cli)
  owns local SBOL file workflows and calls that API; and
- the MCP endpoint gives agents a permission-aware view of the same
  application services.

This separation is intentional. The CLI remains part of the SBOL SDK rather
than becoming a second SBOL DB binary, while registry authorization, ACLs,
identity minting, collision handling, and provenance stay server-authoritative.

## Runtime discovery

`GET /api/v2/instance` includes a `machine_access` object when a public origin
is known:

```json
{
  "machine_access": {
    "api_url": "http://127.0.0.1:8888/api/v2",
    "mcp_url": "http://127.0.0.1:8888/mcp"
  }
}
```

The server derives this origin from its actual bound listener, so a local
server using an ephemeral port advertises that port correctly. Production
deployments behind a reverse proxy must set the externally reachable origin:

```sh
SBOL_DB_PUBLIC_ORIGIN=https://sbol.io sbol-db server
```

The value must be an HTTP or HTTPS origin with no path, credentials, query, or
fragment. Set `SBOL_DB_MCP_ENABLED=false` to leave `/mcp` unmounted and omit
`mcp_url` from discovery.

## `sbol registry`

The `sbol` binary uses the V2 API through the typed
`sbol-registry-client` crate. The initial commands are:

```sh
sbol registry login
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

The compatibility login exchanges the hidden password through `/login` and
stores only the opaque bearer token in the local SBOL profile. The token can
also be supplied to a process as `SBOL_ACCESS_TOKEN`. A future SBOL Identity
issuer can replace this exchange with browser-based authorization without
moving the command or changing the registry operation model.

Push is collision-safe by default. The client always calls
`POST /api/v2/collections/validate` first; the server parses and validates the
content, computes future identities, and reports whether the operation would
create, replace, merge, or conflict. `--preview` stops there without a write.
Commit calls `POST /api/v2/collections` with the same request. The default
collision policy is `fail`; `replace` and `merge` must be chosen explicitly.

## MCP endpoint

`POST /mcp` implements the MCP
[Streamable HTTP transport](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports)
without server-side sessions. Each request is one JSON-RPC message and
requires:

- `Content-Type: application/json`;
- an `Accept` header covering both `application/json` and
  `text/event-stream`; and
- `Authorization: Bearer <token>` for an active SBOL DB account.

The server supports MCP protocol versions `2025-11-25`, `2025-06-18`, and
`2025-03-26`. It does not allocate MCP session IDs or open unsolicited SSE
streams; `GET /mcp` therefore returns `405 Method Not Allowed` with `POST` in
the `Allow` header.

The first tool set is deliberately read-only:

| Tool | Capability |
|---|---|
| `search_designs` | Search public, shared, and private designs visible to the authenticated account. |
| `get_design` | Read one complete, visible design record by canonical IRI. |
| `validate_design_upload` | Parse, validate, mint proposed identities, and analyze collisions without writing. |

All three call the same application facade and ACL computation as the V2 API.
A private design is returned to its owner or an authorized collaborator and is
indistinguishable from a missing design to another account. Missing, malformed,
expired, and unknown bearer credentials are all rejected instead of being
downgraded to anonymous access.

The validation tool requires an active member account but still performs no
write. Mutating agent tools are withheld until the identity layer can issue
audience-bound OAuth tokens with explicit scopes and the MCP surface has a
reviewable confirmation contract.

## Authentication evolution

The current bearer tokens preserve compatibility with SBOL DB and SynBioHub
login behavior. They authenticate the user but are not yet a complete
cross-application authorization system.

“Sign in with SBOL” should add an OAuth 2.1 authorization server to SBOL DB,
with authorization-code plus PKCE for CLI, web, desktop, and agent clients.
That provider must include durable client registration, consent, short-lived
audience-bound access tokens, refresh-token rotation, revocation, and the
standard MCP
[authorization-server and protected-resource metadata](https://modelcontextprotocol.io/specification/2025-11-25/basic/authorization).
Only then should
`machine_access.authorization_issuer` be advertised and mutating MCP tools be
enabled. Flapjack, SBOL Canvas, and the SBOL DB Python client can consume that
issuer without sharing passwords or registry session tokens.

Tracked two-way file synchronization is a separate integrity boundary from
pull and push. It needs a local tracking record, a stable remote revision or
ETag, three-way change detection, and an `If-Match`-style commit guard. Until
those pieces exist, `pull` plus submission `--preview` is the safe supported
workflow; an untracked “sync” alias to destructive replace would not provide
the semantics its name promises.

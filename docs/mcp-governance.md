# MCP compatibility and governance

SBOL DB exposes biological design data to agents through a deliberately small,
versioned MCP surface. The MCP adapter is not a second business-logic layer:
resources and tools call the same application services, authorization rules,
content validators, and compare-and-swap storage operations as the V2 API.

This document is the compatibility contract for maintainers and client
authors. The user-facing connection and capability guide lives in
[CLI and agent access](machine-access.md).

## Protocol and transport baseline

The canonical protocol baseline is MCP `2025-11-25` over stateless Streamable
HTTP at `/mcp`. For client migration, the server currently accepts
`2025-06-18` and `2025-03-26` and negotiates the requested supported version.
Removing a fallback version is a breaking change and requires a release note
and a deprecation window.

Each request is one JSON-RPC 2.0 message. The server advertises tools and
resources, returns no body for notifications, and does not allocate session
IDs. Unknown methods and malformed parameters use JSON-RPC errors. Failures
inside a valid tool call use an MCP tool result with `isError: true` so an agent
can reason about the failure without confusing it with a transport error.

## Authentication and authorization invariants

Every non-loopback deployment uses HTTPS and SBOL Identity OAuth authorization
with PKCE. An MCP token must:

- be issued by this registry's configured issuer;
- have the exact MCP URL as its audience;
- identify both an SBOL account and the OAuth client;
- carry the scopes required by the requested resource or tool; and
- remain subject to the registry's current ownership and sharing rules.

API and UserInfo tokens cannot be replayed at MCP. Private objects are filtered
before serialization and appear missing to unauthorized callers. Scope checks
are additive to object-level authorization, never a replacement for it.

## Resource stability

Static resource URIs are stable identifiers:

| URI | Meaning |
|---|---|
| `sbol://registry` | Connected registry and delegated capability context |
| `sbol://account` | Signed-in SBOL account |
| `sbol://reviews` | ACL-scoped review queue |

Parameterized resources use URI templates advertised by
`resources/templates/list`:

| Template | Meaning |
|---|---|
| `sbol://design{?iri}` | Normalized visible design record |
| `sbol://design-content{?iri,format}` | Serialized visible design closure |

Existing URIs, template parameters, and MIME types cannot be repurposed.
Additive response fields are compatible. Removing a resource, changing its
authorization meaning, or making an optional parameter required is breaking.

## Tool evolution

Tool names describe user capabilities rather than internal repository methods.
Every catalog entry must provide:

- a unique stable name and plain-language description;
- an object-shaped JSON input schema with no undeclared destructive default;
- the exact required OAuth scopes in `requiredScopes` metadata; and
- read-only, destructive, and idempotence annotations consistent with runtime
  behavior.

Adding an optional input or output field is compatible. Renaming a tool,
changing a default effect, narrowing visibility, or changing a required field
is breaking. A replaced tool remains as a documented compatibility alias for
at least one release line when doing so does not weaken security.

## Prepared changes

All registry mutations are two-step workflows:

1. a `prepare_*` tool validates and authorizes an exact proposed payload and
   returns a human-reviewable effect plus a short-lived opaque token;
2. `apply_prepared_change` consumes that token once and applies only the stored
   payload.

The durable preparation record stores a token hash, payload hash, mutation
kind, expiry, account, OAuth client, audience, and required scopes. The raw
token is returned once and is never persisted. Apply fails closed for expiry,
replay, principal mismatch, missing scope, payload corruption, or a stale
design/content baseline. Principal or scope mismatch does not consume the
record. An expired record or an authenticated apply attempt is consumed before
effect dispatch, so a stale baseline or downstream failure cannot be replayed
and must be prepared again; no partial biological change is committed.

Complete collection replacement uses the biological-content ETag as its
baseline. Metadata, sharing, review, audit, and other server-owned state do not
change that ETag. Initial creation remains create-only by default and fails on
identity collision.

## Required conformance gates

A release that changes MCP must pass all of the following:

1. transport lifecycle tests for initialize, notifications, ping, method
   errors, content negotiation, and supported protocol versions;
2. catalog tests for unique names, object schemas, scope metadata, and mutation
   annotations;
3. OAuth tests for protected-resource discovery, exact audience, incremental
   scope challenges, and cross-client/account rejection;
4. ACL tests proving public, owner, collaborator, and unrelated-account views;
5. prepared-change tests for one-time use, expiry, principal binding, stale
   baselines, and atomic application;
6. resource tests for list, templates, reads, MIME types, and private-data
   filtering; and
7. a loopback cross-repository scenario using the real `sbol` CLI to sign in,
   create a private collection, synchronize it with ETags, and prove anonymous
   access remains unavailable.

The final gate is implemented by
[`tests/machine-access/loopback.py`](../tests/machine-access/loopback.py). It
also prepares and applies an MCP collection change, rejects replay of the
one-time token, and verifies that a second CLI checkout detects the resulting
remote-only state.

The repository integration tests are the executable source of truth. Passing
an external MCP inspector is useful interoperability evidence, but it does not
replace the OAuth, ACL, or mutation-safety gates above.

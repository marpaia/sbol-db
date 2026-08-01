"""Protocol-contract tests for the typed Sign in with SBOL helper."""

from __future__ import annotations

import base64
import hashlib
import json
from typing import Dict, List
from urllib.parse import parse_qs

import httpx
import pytest

from sbol_db import (
    IdentityProviderMetadata,
    PublicClientRegistration,
    SbolIdentityClient,
    SbolIdentityError,
)

ISSUER = "https://sbol.io"
REDIRECT = "https://canvas.example/oauth/callback"


def metadata(issuer: str = ISSUER) -> Dict[str, object]:
    return {
        "issuer": issuer,
        "authorization_endpoint": ISSUER + "/oauth/authorize",
        "token_endpoint": ISSUER + "/oauth/token",
        "userinfo_endpoint": ISSUER + "/oauth/userinfo",
        "jwks_uri": ISSUER + "/oauth/jwks",
        "registration_endpoint": ISSUER + "/oauth/register",
        "revocation_endpoint": ISSUER + "/oauth/revoke",
        "scopes_supported": ["openid", "profile", "email", "sbol:read", "sbol:write"],
        "code_challenge_methods_supported": ["S256"],
        "token_endpoint_auth_methods_supported": ["none"],
    }


def test_sign_in_with_sbol_public_client_flow() -> None:
    seen: List[str] = []

    def handler(request: httpx.Request) -> httpx.Response:
        seen.append(request.method + " " + request.url.path)
        if request.url.path == "/.well-known/openid-configuration":
            return httpx.Response(200, json=metadata())
        if request.url.path == "/oauth/register":
            body = json.loads(request.content)
            assert body["token_endpoint_auth_method"] == "none"
            assert "client_secret" not in body
            return httpx.Response(
                201,
                json={
                    "client_id": "client-123",
                    "client_name": body["client_name"],
                    "redirect_uris": body["redirect_uris"],
                },
            )
        if request.url.path == "/oauth/token":
            form = parse_qs(request.content.decode())
            assert form["client_id"] == ["client-123"]
            if form["grant_type"] == ["authorization_code"]:
                assert form["code_verifier"]
                return httpx.Response(
                    200,
                    json={
                        "access_token": "access-secret",
                        "refresh_token": "refresh-secret",
                        "token_type": "Bearer",
                        "expires_in": 3600,
                        "scope": "email openid profile",
                        "resource": ISSUER + "/oauth/userinfo",
                        "id_token": "header.claims.signature",
                    },
                )
            assert form["grant_type"] == ["refresh_token"]
            assert form["refresh_token"] == ["refresh-secret"]
            return httpx.Response(
                200,
                json={
                    "access_token": "rotated-access",
                    "refresh_token": "rotated-refresh",
                    "token_type": "Bearer",
                    "expires_in": 3600,
                    "scope": "openid profile",
                    "resource": ISSUER + "/oauth/userinfo",
                },
            )
        if request.url.path == "/oauth/userinfo":
            assert request.headers["authorization"] == "Bearer access-secret"
            return httpx.Response(200, json={"sub": "user-123", "name": "Alice Example"})
        if request.url.path == "/oauth/revoke":
            assert parse_qs(request.content.decode())["token"] == ["rotated-refresh"]
            return httpx.Response(200)
        raise AssertionError("unexpected request: " + str(request.url))

    with SbolIdentityClient(ISSUER, transport=httpx.MockTransport(handler)) as identity:
        provider = identity.discover()
        registration = identity.register("SBOL Canvas", [REDIRECT], metadata=provider)
        request = identity.begin_authorization(registration, REDIRECT, metadata=provider)

        query = parse_qs(httpx.URL(request.url).query.decode())
        assert query["client_id"] == ["client-123"]
        assert query["code_challenge_method"] == ["S256"]
        expected_challenge = (
            base64.urlsafe_b64encode(hashlib.sha256(request.code_verifier.encode()).digest()).rstrip(b"=").decode()
        )
        assert query["code_challenge"] == [expected_challenge]
        assert request.nonce is not None
        assert query["nonce"] == [request.nonce]

        code = request.code_from_callback(REDIRECT + "?code=code-123&state=" + request.state)
        tokens = identity.exchange_code(request, code, metadata=provider)
        assert tokens.scopes == ("email", "openid", "profile")
        assert "access-secret" not in repr(tokens)
        assert "refresh-secret" not in repr(tokens)
        assert identity.userinfo(tokens.access_token, metadata=provider)["sub"] == "user-123"
        assert tokens.refresh_token is not None

        rotated = identity.refresh(
            registration.client_id,
            tokens.refresh_token,
            resource=ISSUER + "/oauth/userinfo",
            scopes=("openid", "profile"),
            metadata=provider,
        )
        assert rotated.refresh_token == "rotated-refresh"
        assert rotated.refresh_token is not None
        identity.revoke(rotated.refresh_token, metadata=provider)

    assert seen.count("GET /.well-known/openid-configuration") == 1
    assert "POST /oauth/register" in seen
    assert seen.count("POST /oauth/token") == 2


def test_authorization_callback_is_redirect_and_state_bound() -> None:
    metadata_value = IdentityProviderMetadata.from_json(metadata())
    registration = PublicClientRegistration("client-123", (REDIRECT,), "Canvas")
    with SbolIdentityClient(ISSUER, transport=httpx.MockTransport(lambda _request: httpx.Response(500))) as client:
        request = client.begin_authorization(registration, REDIRECT, metadata=metadata_value)

        with pytest.raises(SbolIdentityError, match="state did not match"):
            request.code_from_callback(REDIRECT + "?code=code-123&state=wrong")
        with pytest.raises(SbolIdentityError, match="registered redirect URI"):
            request.code_from_callback("https://evil.example/callback?code=code-123&state=" + request.state)
        with pytest.raises(SbolIdentityError, match="authorization failed: user cancelled"):
            request.code_from_callback(
                REDIRECT + "?error=access_denied&error_description=user+cancelled&state=" + request.state
            )

        redirect_with_query = REDIRECT + "?tenant=sbol"
        queried = client.begin_authorization(
            PublicClientRegistration("client-456", (redirect_with_query,), "Flapjack"),
            redirect_with_query,
            metadata=metadata_value,
        )
        with pytest.raises(SbolIdentityError, match="registered redirect query"):
            queried.code_from_callback(REDIRECT + "?code=code-456&state=" + queried.state)
        assert queried.code_from_callback(redirect_with_query + "&code=code-456&state=" + queried.state) == "code-456"


def test_discovery_rejects_issuer_substitution() -> None:
    def handler(_request: httpx.Request) -> httpx.Response:
        return httpx.Response(200, json=metadata("https://evil.example"))

    with SbolIdentityClient(ISSUER, transport=httpx.MockTransport(handler)) as identity:
        with pytest.raises(SbolIdentityError, match="issuer mismatch"):
            identity.discover()


def test_registry_capability_request_is_resource_bound_without_oidc_nonce() -> None:
    provider = IdentityProviderMetadata.from_json(metadata())
    registration = PublicClientRegistration("client-123", (REDIRECT,), "CLI-like client")
    with SbolIdentityClient(ISSUER, transport=httpx.MockTransport(lambda _request: httpx.Response(500))) as identity:
        request = identity.begin_authorization(
            registration,
            REDIRECT,
            scopes=("sbol:read", "sbol:write"),
            resource=ISSUER + "/api/v2",
            metadata=provider,
        )
    query = parse_qs(httpx.URL(request.url).query.decode())
    assert query["resource"] == [ISSUER + "/api/v2"]
    assert request.nonce is None

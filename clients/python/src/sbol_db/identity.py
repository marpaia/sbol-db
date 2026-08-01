"""Typed OAuth/OIDC primitives for "Sign in with SBOL".

The helper owns protocol-sensitive client work that ecosystem applications
should not reproduce ad hoc: exact-issuer discovery, public-client dynamic
registration, S256 PKCE, state and callback validation, resource-bound token
exchange, refresh rotation, revocation, and UserInfo. It deliberately leaves
browser routing and durable credential storage to the host application.
"""

from __future__ import annotations

import base64
import hashlib
import secrets
from dataclasses import dataclass, field
from typing import Any, Dict, Iterable, Mapping, Optional, Tuple
from urllib.parse import parse_qs, urlencode, urlsplit, urlunsplit

import httpx


class SbolIdentityError(RuntimeError):
    """A discovery, authorization, callback, or token-contract failure."""


@dataclass(frozen=True)
class IdentityProviderMetadata:
    """The SBOL Identity subset of OpenID Provider metadata."""

    issuer: str
    authorization_endpoint: str
    token_endpoint: str
    userinfo_endpoint: str
    jwks_uri: str
    registration_endpoint: str
    revocation_endpoint: Optional[str]
    scopes_supported: Tuple[str, ...]
    code_challenge_methods_supported: Tuple[str, ...]
    token_endpoint_auth_methods_supported: Tuple[str, ...]

    @classmethod
    def from_json(cls, value: Mapping[str, Any]) -> "IdentityProviderMetadata":
        required = (
            "issuer",
            "authorization_endpoint",
            "token_endpoint",
            "userinfo_endpoint",
            "jwks_uri",
            "registration_endpoint",
        )
        missing = [name for name in required if not isinstance(value.get(name), str)]
        if missing:
            raise SbolIdentityError("identity metadata is missing: " + ", ".join(missing))
        return cls(
            issuer=str(value["issuer"]),
            authorization_endpoint=str(value["authorization_endpoint"]),
            token_endpoint=str(value["token_endpoint"]),
            userinfo_endpoint=str(value["userinfo_endpoint"]),
            jwks_uri=str(value["jwks_uri"]),
            registration_endpoint=str(value["registration_endpoint"]),
            revocation_endpoint=_optional_string(value.get("revocation_endpoint")),
            scopes_supported=_string_tuple(value.get("scopes_supported")),
            code_challenge_methods_supported=_string_tuple(value.get("code_challenge_methods_supported")),
            token_endpoint_auth_methods_supported=_string_tuple(value.get("token_endpoint_auth_methods_supported")),
        )


@dataclass(frozen=True)
class PublicClientRegistration:
    """A dynamically registered OAuth public client (there is no secret)."""

    client_id: str
    redirect_uris: Tuple[str, ...]
    client_name: Optional[str] = None

    @classmethod
    def from_json(cls, value: Mapping[str, Any]) -> "PublicClientRegistration":
        client_id = value.get("client_id")
        if not isinstance(client_id, str) or not client_id:
            raise SbolIdentityError("client registration did not return a client_id")
        return cls(
            client_id=client_id,
            redirect_uris=_string_tuple(value.get("redirect_uris")),
            client_name=_optional_string(value.get("client_name")),
        )


@dataclass(frozen=True)
class OAuthTokens:
    """A scoped token response whose credentials are redacted from ``repr``."""

    access_token: str = field(repr=False)
    token_type: str = "Bearer"
    expires_in: int = 0
    scopes: Tuple[str, ...] = ()
    resource: Optional[str] = None
    refresh_token: Optional[str] = field(default=None, repr=False)
    id_token: Optional[str] = field(default=None, repr=False)

    @classmethod
    def from_json(cls, value: Mapping[str, Any]) -> "OAuthTokens":
        access_token = value.get("access_token")
        token_type = value.get("token_type")
        if not isinstance(access_token, str) or not access_token:
            raise SbolIdentityError("token response did not include an access_token")
        if not isinstance(token_type, str) or token_type.lower() != "bearer":
            raise SbolIdentityError("token response did not return a Bearer access token")
        expires_in = value.get("expires_in", 0)
        if not isinstance(expires_in, int) or isinstance(expires_in, bool) or expires_in < 0:
            raise SbolIdentityError("token response expires_in must be a non-negative integer")
        scope = value.get("scope", "")
        if not isinstance(scope, str):
            raise SbolIdentityError("token response scope must be a string")
        return cls(
            access_token=access_token,
            token_type=token_type,
            expires_in=expires_in,
            scopes=tuple(sorted(set(scope.split()))),
            resource=_optional_string(value.get("resource")),
            refresh_token=_optional_string(value.get("refresh_token")),
            id_token=_optional_string(value.get("id_token")),
        )


@dataclass(frozen=True)
class AuthorizationRequest:
    """A browser authorization URL plus the secrets needed for its callback."""

    url: str
    client_id: str
    redirect_uri: str
    scopes: Tuple[str, ...]
    resource: Optional[str]
    code_verifier: str = field(repr=False)
    state: str = field(repr=False)
    nonce: Optional[str] = field(default=None, repr=False)

    def code_from_callback(self, callback_url: str) -> str:
        """Validate the exact callback and state, then return its code."""

        callback = urlsplit(callback_url)
        expected = urlsplit(self.redirect_uri)
        callback_target = urlunsplit((callback.scheme, callback.netloc, callback.path, "", ""))
        expected_target = urlunsplit((expected.scheme, expected.netloc, expected.path, "", ""))
        if callback_target != expected_target:
            raise SbolIdentityError("authorization callback did not use the registered redirect URI")
        query = parse_qs(callback.query, keep_blank_values=True)
        expected_query = parse_qs(expected.query, keep_blank_values=True)
        if any(query.get(name) != values for name, values in expected_query.items()):
            raise SbolIdentityError("authorization callback omitted the registered redirect query")
        returned_state = _single_query_value(query, "state")
        if not secrets.compare_digest(returned_state, self.state):
            raise SbolIdentityError("authorization callback state did not match")
        error = query.get("error")
        if error:
            description = query.get("error_description", error)[0]
            raise SbolIdentityError("authorization failed: " + description)
        code = _single_query_value(query, "code")
        if not code:
            raise SbolIdentityError("authorization callback did not include a code")
        return code


class SbolIdentityClient:
    """A synchronous public client for one SBOL Identity issuer."""

    def __init__(
        self,
        issuer: str,
        *,
        timeout: float = 30.0,
        transport: Optional[httpx.BaseTransport] = None,
    ) -> None:
        self.issuer = issuer.rstrip("/")
        _validate_secure_url(self.issuer, "issuer")
        self._client = httpx.Client(timeout=timeout, transport=transport)

    def close(self) -> None:
        self._client.close()

    def __enter__(self) -> "SbolIdentityClient":
        return self

    def __exit__(self, *_exc: object) -> None:
        self.close()

    def discover(self) -> IdentityProviderMetadata:
        """Discover and validate the issuer's OpenID Provider metadata."""

        url = self.issuer + "/.well-known/openid-configuration"
        value = self._json(self._client.get(url, headers={"accept": "application/json"}))
        metadata = IdentityProviderMetadata.from_json(value)
        if metadata.issuer != self.issuer:
            raise SbolIdentityError(
                "identity metadata issuer mismatch: expected %s, received %s" % (self.issuer, metadata.issuer)
            )
        for name, endpoint in (
            ("authorization endpoint", metadata.authorization_endpoint),
            ("token endpoint", metadata.token_endpoint),
            ("UserInfo endpoint", metadata.userinfo_endpoint),
            ("JWKS URI", metadata.jwks_uri),
            ("registration endpoint", metadata.registration_endpoint),
        ):
            _validate_secure_url(endpoint, name)
        if metadata.revocation_endpoint is not None:
            _validate_secure_url(metadata.revocation_endpoint, "revocation endpoint")
        if "S256" not in metadata.code_challenge_methods_supported:
            raise SbolIdentityError("SBOL Identity provider does not advertise S256 PKCE")
        if "none" not in metadata.token_endpoint_auth_methods_supported:
            raise SbolIdentityError("SBOL Identity provider does not advertise public clients")
        return metadata

    def register(
        self,
        client_name: str,
        redirect_uris: Iterable[str],
        *,
        metadata: Optional[IdentityProviderMetadata] = None,
    ) -> PublicClientRegistration:
        """Dynamically register an authorization-code public client."""

        metadata = metadata or self.discover()
        redirects = tuple(redirect_uris)
        if not client_name.strip():
            raise ValueError("client_name must not be empty")
        if not redirects:
            raise ValueError("at least one redirect URI is required")
        for redirect in redirects:
            _validate_redirect_uri(redirect)
        value = self._json(
            self._client.post(
                metadata.registration_endpoint,
                headers={"accept": "application/json"},
                json={
                    "client_name": client_name,
                    "redirect_uris": list(redirects),
                    "grant_types": ["authorization_code", "refresh_token"],
                    "response_types": ["code"],
                    "token_endpoint_auth_method": "none",
                },
            )
        )
        registration = PublicClientRegistration.from_json(value)
        if registration.redirect_uris != redirects:
            raise SbolIdentityError("client registration returned different redirect URIs")
        return registration

    def begin_authorization(
        self,
        registration: PublicClientRegistration,
        redirect_uri: str,
        *,
        scopes: Iterable[str] = ("openid", "profile", "email"),
        resource: Optional[str] = None,
        metadata: Optional[IdentityProviderMetadata] = None,
    ) -> AuthorizationRequest:
        """Create a state-, nonce-, resource-, and S256-bound browser request."""

        metadata = metadata or self.discover()
        if registration.redirect_uris and redirect_uri not in registration.redirect_uris:
            raise ValueError("redirect_uri is not part of this client registration")
        requested = tuple(sorted(set(scope.strip() for scope in scopes if scope.strip())))
        if not requested:
            raise ValueError("at least one scope is required")
        unsupported = set(requested).difference(metadata.scopes_supported)
        if unsupported:
            raise ValueError("unsupported identity scopes: " + ", ".join(sorted(unsupported)))
        if ("profile" in requested or "email" in requested) and "openid" not in requested:
            raise ValueError("profile and email scopes require openid")
        if resource is not None:
            _validate_secure_url(resource, "protected resource")
        verifier = _random_urlsafe(32)
        challenge = _base64url(hashlib.sha256(verifier.encode("ascii")).digest())
        state = _random_urlsafe(32)
        nonce = _random_urlsafe(32) if "openid" in requested else None
        query = {
            "response_type": "code",
            "client_id": registration.client_id,
            "redirect_uri": redirect_uri,
            "code_challenge": challenge,
            "code_challenge_method": "S256",
            "scope": " ".join(requested),
            "state": state,
        }
        if resource is not None:
            query["resource"] = resource
        if nonce is not None:
            query["nonce"] = nonce
        separator = "&" if urlsplit(metadata.authorization_endpoint).query else "?"
        return AuthorizationRequest(
            url=metadata.authorization_endpoint + separator + urlencode(query),
            client_id=registration.client_id,
            redirect_uri=redirect_uri,
            scopes=requested,
            resource=resource,
            code_verifier=verifier,
            state=state,
            nonce=nonce,
        )

    def exchange_code(
        self,
        request: AuthorizationRequest,
        code: str,
        *,
        metadata: Optional[IdentityProviderMetadata] = None,
    ) -> OAuthTokens:
        """Exchange one authorization code using its original PKCE binding."""

        metadata = metadata or self.discover()
        data = {
            "grant_type": "authorization_code",
            "code": code,
            "client_id": request.client_id,
            "redirect_uri": request.redirect_uri,
            "code_verifier": request.code_verifier,
        }
        if request.resource is not None:
            data["resource"] = request.resource
        tokens = self._tokens(metadata.token_endpoint, data)
        expected_resource = request.resource
        if expected_resource is None and "openid" in request.scopes:
            expected_resource = metadata.userinfo_endpoint
        if expected_resource is not None and tokens.resource != expected_resource:
            raise SbolIdentityError("token response was issued for the wrong protected resource")
        if not set(request.scopes).issubset(tokens.scopes):
            raise SbolIdentityError("token response omitted one or more approved scopes")
        return tokens

    def refresh(
        self,
        client_id: str,
        refresh_token: str,
        *,
        resource: Optional[str] = None,
        scopes: Iterable[str] = (),
        metadata: Optional[IdentityProviderMetadata] = None,
    ) -> OAuthTokens:
        """Rotate a refresh token, optionally narrowing its scopes."""

        metadata = metadata or self.discover()
        data = {
            "grant_type": "refresh_token",
            "refresh_token": refresh_token,
            "client_id": client_id,
        }
        requested = tuple(sorted(set(scope.strip() for scope in scopes if scope.strip())))
        if resource is not None:
            data["resource"] = resource
        if requested:
            data["scope"] = " ".join(requested)
        tokens = self._tokens(metadata.token_endpoint, data)
        if resource is not None and tokens.resource != resource:
            raise SbolIdentityError("refreshed token was issued for the wrong protected resource")
        if requested and not set(requested).issubset(tokens.scopes):
            raise SbolIdentityError("refreshed token omitted one or more requested scopes")
        return tokens

    def userinfo(
        self,
        access_token: str,
        *,
        metadata: Optional[IdentityProviderMetadata] = None,
    ) -> Dict[str, Any]:
        """Resolve scoped OpenID claims for a UserInfo audience token."""

        metadata = metadata or self.discover()
        return self._json(
            self._client.get(
                metadata.userinfo_endpoint,
                headers={"accept": "application/json", "authorization": "Bearer " + access_token},
            )
        )

    def revoke(
        self,
        token: str,
        *,
        metadata: Optional[IdentityProviderMetadata] = None,
    ) -> None:
        """Revoke an access or refresh token without probing its validity."""

        metadata = metadata or self.discover()
        if metadata.revocation_endpoint is None:
            raise SbolIdentityError("SBOL Identity provider does not advertise token revocation")
        response = self._client.post(metadata.revocation_endpoint, data={"token": token})
        self._raise_for_oauth_error(response)

    def _tokens(self, endpoint: str, data: Mapping[str, str]) -> OAuthTokens:
        value = self._json(self._client.post(endpoint, headers={"accept": "application/json"}, data=data))
        return OAuthTokens.from_json(value)

    def _json(self, response: httpx.Response) -> Dict[str, Any]:
        self._raise_for_oauth_error(response)
        try:
            value = response.json()
        except ValueError as error:
            raise SbolIdentityError("SBOL Identity returned invalid JSON") from error
        if not isinstance(value, dict):
            raise SbolIdentityError("SBOL Identity response must be a JSON object")
        return value

    @staticmethod
    def _raise_for_oauth_error(response: httpx.Response) -> None:
        if response.is_success:
            return
        try:
            value = response.json()
        except ValueError:
            value = {}
        if isinstance(value, dict):
            code = value.get("error")
            description = value.get("error_description")
            if isinstance(description, str):
                raise SbolIdentityError("SBOL Identity rejected the request: " + description)
            if isinstance(code, str):
                raise SbolIdentityError("SBOL Identity rejected the request: " + code)
        raise SbolIdentityError("SBOL Identity returned HTTP %d" % response.status_code)


def _validate_secure_url(value: str, label: str) -> None:
    parsed = urlsplit(value)
    loopback = parsed.hostname in ("localhost", "127.0.0.1", "::1")
    if not parsed.hostname or not (parsed.scheme == "https" or (parsed.scheme == "http" and loopback)):
        raise SbolIdentityError("%s must use HTTPS or loopback HTTP" % label)
    if parsed.username is not None or parsed.password is not None or parsed.fragment:
        raise SbolIdentityError("%s cannot contain credentials or a fragment" % label)


def _validate_redirect_uri(value: str) -> None:
    _validate_secure_url(value, "redirect URI")


def _base64url(value: bytes) -> str:
    return base64.urlsafe_b64encode(value).rstrip(b"=").decode("ascii")


def _random_urlsafe(length: int) -> str:
    return _base64url(secrets.token_bytes(length))


def _string_tuple(value: Any) -> Tuple[str, ...]:
    if not isinstance(value, list) or not all(isinstance(item, str) for item in value):
        return ()
    return tuple(value)


def _optional_string(value: Any) -> Optional[str]:
    return value if isinstance(value, str) else None


def _single_query_value(query: Mapping[str, Any], name: str) -> str:
    values = query.get(name)
    if not isinstance(values, list) or len(values) != 1 or not isinstance(values[0], str):
        raise SbolIdentityError("authorization callback must include exactly one " + name)
    return values[0]

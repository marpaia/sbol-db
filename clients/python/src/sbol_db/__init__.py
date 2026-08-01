"""Pure-Python client for the sbol-db HTTP API.

``SbolDbClient`` is the broad client over sbol-db's native REST API;
``PartShop`` is a pysbol2-shaped facade for code migrating from SynBioHub.
"""

from __future__ import annotations

from .client import SbolDbClient
from .errors import (
    BackendUnsupportedError,
    BadRequestError,
    NotFoundError,
    SbolDbError,
    SparqlError,
    TimeoutError_,
)
from .identity import (
    AuthorizationRequest,
    IdentityProviderMetadata,
    OAuthTokens,
    PublicClientRegistration,
    SbolIdentityClient,
    SbolIdentityError,
)
from .models import GraphRecord, ImportReport, SbolObject
from .partshop import PartShop
from .sparql import SparqlResult

__version__ = "0.1.0"

__all__ = [
    "SbolDbClient",
    "PartShop",
    "SbolObject",
    "GraphRecord",
    "ImportReport",
    "SparqlResult",
    "SbolDbError",
    "BadRequestError",
    "NotFoundError",
    "SparqlError",
    "TimeoutError_",
    "BackendUnsupportedError",
    "SbolIdentityClient",
    "SbolIdentityError",
    "IdentityProviderMetadata",
    "PublicClientRegistration",
    "AuthorizationRequest",
    "OAuthTokens",
]

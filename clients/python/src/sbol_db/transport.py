"""HTTP transport shared by the client and the PartShop facade.

Wraps a single :class:`httpx.Client`, centralizes base URL, optional HTTP Basic
auth, timeout, and retries, and turns any non-2xx response into the appropriate
:class:`~sbol_db.errors.SbolDbError`.
"""

from __future__ import annotations

from typing import Any, Mapping, Optional, Tuple

import httpx

from .errors import error_for_response


class Transport:
    """A thin, synchronous HTTP wrapper with typed error mapping."""

    def __init__(
        self,
        base_url: str,
        *,
        auth: Optional[Tuple[str, str]] = None,
        timeout: float = 30.0,
        retries: int = 2,
        transport: Optional[httpx.BaseTransport] = None,
    ) -> None:
        self._client = httpx.Client(
            base_url=base_url.rstrip("/"),
            auth=httpx.BasicAuth(*auth) if auth else None,
            timeout=timeout,
            transport=transport or httpx.HTTPTransport(retries=retries),
        )

    def close(self) -> None:
        self._client.close()

    def __enter__(self) -> "Transport":
        return self

    def __exit__(self, *_exc: object) -> None:
        self.close()

    def request(
        self,
        method: str,
        path: str,
        *,
        params: Optional[Mapping[str, Any]] = None,
        json: Any = None,
        data: Optional[Mapping[str, Any]] = None,
        content: Optional[bytes] = None,
        headers: Optional[Mapping[str, str]] = None,
    ) -> httpx.Response:
        """Issue a request, raising a typed error on any non-2xx response.

        ``params`` (query string) and ``data`` (form body) entries whose value
        is ``None`` are dropped, so callers can pass optional fields
        unconditionally.
        """
        clean_params = None if params is None else {k: v for k, v in params.items() if v is not None}
        clean_data = None if data is None else {k: v for k, v in data.items() if v is not None}
        response = self._client.request(
            method,
            path,
            params=clean_params,
            json=json,
            data=clean_data,
            content=content,
            headers=headers,
        )
        if response.is_success:
            return response
        raise error_for_response(response)

"""Official Python caller SDK for an AIP service.

The server stays the final authority: this client only expresses reads, transitions, atomic bundles of exposed
creates and transitions, composes, operations and durable jobs that the deployed contract exposes. It mirrors the TypeScript
transport rules for contract fingerprints, session scope and unsettled-write recovery.
Only the standard library is used so it runs in scripts, workers and other backends.
"""

from __future__ import annotations

import base64
import copy
import json
import re
import urllib.error
import urllib.parse
import urllib.request
import uuid
from typing import Any, Callable, Iterator, Optional

__all__ = ["connect", "Client", "AipError", "WriteUnsettled", "ScopeConflict"]
__version__ = "0.1.0"

_FINGERPRINT = re.compile(r"^[a-f0-9]{64}$")
_ACTOR_ID = re.compile(r"^(0|[1-9][0-9]*|-[1-9][0-9]*|[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})$")
# A write whose commit could not be confirmed is resent with the same key and body.
_UNSETTLED = {"WIRE.COMMIT_UNKNOWN", "WIRE.INTERNAL"}
# Rejections before execution say nothing about whether an earlier attempt with the key committed.
_NOT_EXECUTED = {
    "WIRE.TOKEN_EXPIRED", "WIRE.UNAUTHENTICATED", "WIRE.BAD_REQUEST", "WIRE.PAYLOAD_TOO_LARGE", "WIRE.NOT_FOUND", "WIRE.CONTRACT_MISMATCH", "WIRE.CONTRACT_REQUIRED",
    "WIRE.ORIGIN_NOT_ALLOWED", "WIRE.REQUEST_TIMEOUT", "WIRE.DB_UNAVAILABLE", "WIRE.WORKER_BUSY", "WIRE.DEADLINE_EXCEEDED", "WIRE.CONFLICT",
}
_CONTRACT_PATHS = {"/read", "/apply", "/status", "/extension", "/operation"}
_MANAGED_PREFIXES = ("/jobs/", "/files/", "/events/read", "/messages/", "/rag/", "/objects/")


class AipError(Exception):
    """A typed server or protocol rejection. `code` is the stable AIP error code. `request_id` is the `requestId` of
    the failure body (the same value as the `x-request-id` response header; None when the reply had neither); report
    it so the operator can find the log line. `retry` is the server's guidance for the code, one of `never`, `later`,
    `check-first` or `reauthenticate` (None when the failure did not come from a server reply)."""

    def __init__(self, code: str, message: Optional[str] = None, request_id: Optional[str] = None, retry: Optional[str] = None):
        super().__init__(message or code)
        self.code = code
        self.request_id = request_id
        self.retry = retry


class WriteUnsettled(AipError):
    """The write may or may not have committed. Retry with `client.retry_pending()` or the same key."""

    def __init__(self, key: str, request: Any, request_id: Optional[str] = None, retry: Optional[str] = None):
        super().__init__("CLIENT.WRITE_UNSETTLED", "쓰기 결과를 아직 확정하지 못함", request_id, retry)
        self.key = key
        self.request = request


class ScopeConflict(AipError):
    def __init__(self) -> None:
        super().__init__("CLIENT.SCOPE_CONFLICT", "다른 사용자 세션으로 미확정 쓰기를 이관할 수 없음")


# A transport returns the body, or `(body, response_headers)` when it can also give the response headers.
Transport = Callable[[str, dict, bytes, float], Any]

_SAFE_REQUEST_ID = re.compile(r"^[A-Za-z0-9._-]{8,64}$")
_RETRY_GUIDANCE = {"never", "later", "check-first", "reauthenticate"}


def _well_formed_bundle_result(response: dict) -> bool:
    """A committed bundle or compose answers `applied` (changed/unchanged ids per operation) and `created`
    (resource and id, `null` for a table without an id column). Anything else is not a result to trust."""
    applied, created = response.get("applied"), response.get("created")
    if not isinstance(applied, list) or not isinstance(created, list):
        return False
    ids_ok = lambda ids: isinstance(ids, list)  # noqa: E731
    return all(isinstance(row, dict) and ids_ok(row.get("changed")) and ids_ok(row.get("unchanged")) for row in applied) and all(
        isinstance(row, dict) and isinstance(row.get("resource"), str) and "id" in row for row in created)


class _Body(dict):
    """A parsed JSON object that remembers the request id of its reply. The body itself is the server's, unchanged."""

    request_id: Optional[str] = None
    retry: Optional[str] = None


def _retry(response: Any) -> Optional[str]:
    value = response.get("retry") if isinstance(response, dict) else None
    return value if isinstance(value, str) and value in _RETRY_GUIDANCE else None


def _request_id(response: Any) -> Optional[str]:
    return getattr(response, "request_id", None)


def _reply_request_id(headers: Any) -> Optional[str]:
    if not headers:
        return None
    value = next((v for k, v in dict(headers).items() if str(k).lower() == "x-request-id"), None)
    return value if isinstance(value, str) and _SAFE_REQUEST_ID.match(value) else None


class _NoRedirect(urllib.request.HTTPRedirectHandler):
    """urllib keeps the Authorization header across redirects, including to another origin. An AIP service never
    redirects an API call, so a 3xx is refused instead of followed and the bearer token stays with the configured origin."""

    def redirect_request(self, *args: Any, **kwargs: Any) -> None:
        return None


_OPENER = urllib.request.build_opener(_NoRedirect)
_LOOPBACK_HOSTS = {"localhost", "127.0.0.1", "::1"}


def _check_url(url: str) -> None:
    parts = urllib.parse.urlsplit(url)
    if parts.scheme == "https":
        return
    # The service listens on loopback behind a TLS proxy; a bearer token must not cross a network in clear text.
    if parts.scheme == "http" and parts.hostname in _LOOPBACK_HOSTS:
        return
    raise AipError("CLIENT.PROTOCOL_ERROR", "https 또는 loopback http 주소만 사용할 수 있음")


def _urllib_transport(url: str, headers: dict, body: bytes, timeout: float) -> Any:
    _check_url(url)
    request = urllib.request.Request(url, data=body, headers=headers, method="POST")
    try:
        with _OPENER.open(request, timeout=timeout) as response:
            return response.read(), {"x-request-id": response.headers.get("x-request-id")}
    except urllib.error.HTTPError as error:
        # Webhook-style status codes still carry the JSON envelope. A refused redirect has no JSON and ends as PROTOCOL_ERROR.
        with error:
            return error.read(), {"x-request-id": error.headers.get("x-request-id")}


class Client:
    def __init__(self, base_url: str, token: Optional[str], contract_fingerprint: Optional[str] = None,
                 timeout: float = 30.0, transport: Optional[Transport] = None):
        if contract_fingerprint is not None and not _FINGERPRINT.match(contract_fingerprint):
            raise AipError("CLIENT.PROTOCOL_ERROR", "계약 식별값 형식 오류")
        self._base = base_url.rstrip("/")
        self._token = token
        self._pinned = contract_fingerprint is not None
        self.contract_fingerprint = contract_fingerprint
        self._timeout = timeout
        self._transport = transport or _urllib_transport
        self._principal: Any = ...
        self._pending: dict[str, Any] = {}
        self.jobs = _Jobs(self)
        self.files = _Files(self)
        self.events = _Events(self)

    # -- wire ---------------------------------------------------------------------------------
    def _post(self, path: str, body: Any, token: Any = ..., contract: bool = True) -> Any:
        credential = self._token if token is ... else token
        headers = {"content-type": "application/json"}
        if credential:
            headers["authorization"] = f"Bearer {credential}"
        if contract and self.contract_fingerprint and (path in _CONTRACT_PATHS or path.startswith(_MANAGED_PREFIXES)):
            headers["x-aip-contract"] = self.contract_fingerprint
        raw = self._transport(self._base + path, headers, json.dumps(body, ensure_ascii=False).encode("utf-8"), self._timeout)
        request_id = None
        if isinstance(raw, tuple):
            raw, headers_in = raw
            request_id = _reply_request_id(headers_in)
        try:
            parsed = json.loads(raw)
        except (ValueError, TypeError):
            raise AipError("CLIENT.PROTOCOL_ERROR", "JSON 응답이 아님", request_id) from None
        if isinstance(parsed, dict):
            parsed = _Body(parsed)
            # The failure body names its request (same value as the header) and says whether to send it again.
            body_id = parsed.get("requestId")
            parsed.request_id = body_id if isinstance(body_id, str) and _SAFE_REQUEST_ID.match(body_id) else request_id
            if parsed.get("ok") is False and isinstance(parsed.get("retry"), str) and parsed["retry"] in _RETRY_GUIDANCE:
                parsed.retry = parsed["retry"]
        return parsed

    def _checked(self, response: Any, expect_contract: bool) -> dict:
        if isinstance(response, dict) and response.get("ok") is False and isinstance(response.get("code"), str):
            raise AipError(response["code"], response.get("msg") if isinstance(response.get("msg"), str) else None, _request_id(response), _retry(response))
        if not isinstance(response, dict) or response.get("ok") is not True:
            raise AipError("CLIENT.PROTOCOL_ERROR", "응답 형식 오류", _request_id(response))
        if expect_contract and response.get("contractFingerprint") != self.contract_fingerprint:
            raise AipError("WIRE.CONTRACT_MISMATCH", "고정한 계약과 서버 계약이 다름", _request_id(response))
        return response

    def _describe(self, token: Optional[str]) -> Optional[str]:
        response = self._post("/session", {}, token=token, contract=False)
        if isinstance(response, dict) and response.get("ok") is False and isinstance(response.get("code"), str):
            raise AipError(response["code"], None, _request_id(response), _retry(response))
        actor = (response.get("principal") or {}).get("actorId") if isinstance(response, dict) else None
        if response.get("ok") is not True or (token and not (isinstance(actor, str) and _ACTOR_ID.match(actor))):
            raise AipError("CLIENT.PROTOCOL_ERROR", "세션 응답 계약 오류", _request_id(response))
        return actor

    def _ensure_principal(self) -> Optional[str]:
        if self._principal is ...:
            self._principal = self._describe(self._token)
        return self._principal

    def _ensure_contract(self) -> None:
        if self.contract_fingerprint is None:
            self.discover()

    # -- discovery ----------------------------------------------------------------------------
    def discover(self) -> dict:
        """Authenticated capability catalog. Without a pinned fingerprint, adopts the server's."""
        self._ensure_principal()
        response = self._checked(self._post("/capabilities", {}, contract=False), expect_contract=False)
        served = response.get("contractFingerprint")
        if not isinstance(served, str) or not _FINGERPRINT.match(served):
            raise AipError("CLIENT.PROTOCOL_ERROR", "계약 식별값이 없음", _request_id(response))
        if self._pinned and served != self.contract_fingerprint:
            raise AipError("WIRE.CONTRACT_MISMATCH", "고정한 계약과 서버 계약이 다름", _request_id(response))
        self.contract_fingerprint = served
        return response

    # -- data ---------------------------------------------------------------------------------
    def read(self, query: dict) -> list:
        self._ensure_principal()
        self._ensure_contract()
        response = self._checked(self._post("/read", {"query": copy.deepcopy(query)}), expect_contract=True)
        if not isinstance(response.get("rows"), list):
            raise AipError("CLIENT.PROTOCOL_ERROR", "읽기 응답 계약 오류", _request_id(response))
        return response["rows"]

    def apply(self, request: dict, key: Optional[str] = None, retries: int = 2) -> dict:
        """Runs an exposed transition or write extension. A lost or unconfirmed response is resent with
        the same key; if it still cannot be confirmed, `WriteUnsettled` keeps the key for later retry."""
        key = key or str(uuid.uuid4())
        snapshot = json.loads(json.dumps(request))
        self._ensure_principal()
        self._ensure_contract()
        previous = self._pending.get(key)
        if previous is not None and previous != snapshot:
            return {"ok": False, "code": "WIRE.IDEMPOTENCY_MISMATCH"}
        self._pending[key] = snapshot
        last_request_id: Optional[str] = None
        last_retry: Optional[str] = None
        for attempt in range(retries + 1):
            try:
                response = self._post("/apply", {"request": snapshot, "key": key})
            except (OSError, AipError) as failure:
                last_request_id = getattr(failure, "request_id", None) or last_request_id
                continue
            last_retry = _retry(response) or last_retry
            last_request_id = _request_id(response) or last_request_id
            code = response.get("code") if isinstance(response, dict) else None
            if not isinstance(response, dict) or (response.get("ok") is not True and not isinstance(code, str)):
                continue
            if code in _UNSETTLED or (attempt > 0 and code in _NOT_EXECUTED):
                continue
            # A bundle or compose that reports success must say what it changed and created; otherwise the commit is
            # unconfirmed and the same key is sent again to read the stored result.
            if response.get("ok") is True and ("atomic" in snapshot or "compose" in snapshot) and not _well_formed_bundle_result(response):
                continue
            del self._pending[key]
            request_id = _request_id(response)
            if attempt > 0:
                response = {**response, "recovered": True}
            # A failed outcome names its request so that the rejection can be reported; a success carries nothing extra.
            if request_id and response.get("ok") is False and "requestId" not in response:
                response = {**response, "requestId": request_id}
            if response.get("ok") is False and "retry" in response and _retry(response) is None:
                response = {k: v for k, v in response.items() if k != "retry"}
            return response
        raise WriteUnsettled(key, copy.deepcopy(snapshot), last_request_id, last_retry)

    def bundle(self, operations: list, key: Optional[str] = None, retries: int = 2) -> dict:
        """Runs exposed transitions (`{"apply": "R.t", "target": {...}}`) and exposed creates
        (`{"create": "R", "values": {...}}`) as one atomic request: all succeed or nothing changes. The result is
        `{"ok": True, "applied": [{"changed", "unchanged"}...], "created": [{"resource", "id"}...], "tags"}` or a
        rejection. The key, lost-response recovery and `WriteUnsettled` work as for `apply`."""
        if not isinstance(operations, (list, tuple)) or not operations:
            raise AipError("WIRE.BAD_REQUEST", "묶음에는 연산이 하나 이상 있어야 함")
        return self.apply({"atomic": list(operations)}, key=key, retries=retries)

    def compose(self, resource: str, ids: list, steps: list, key: Optional[str] = None, retries: int = 2) -> dict:
        """Runs an exposed compose: the steps (`{"transition": "t"}` or `{"create": "R", "values": {field: {"item":
        "path"} | {"const": value}}}`) apply to every target id. Values come from the target row the server reads."""
        if not isinstance(steps, (list, tuple)) or not steps:
            raise AipError("WIRE.BAD_REQUEST", "조합에는 단계가 하나 이상 있어야 함")
        return self.apply({"compose": resource, "targets": {"ids": list(ids)}, "steps": list(steps)}, key=key, retries=retries)

    def status(self, request: dict, key: str) -> dict:
        """Committed result for a key and identical request, or `WIRE.NOT_FOUND` if none is committed yet."""
        self._ensure_contract()
        return self._post("/status", {"request": request, "key": key})

    def pending(self) -> list:
        return list(self._pending)

    def retry_pending(self) -> list:
        settled = []
        for key, request in list(self._pending.items()):
            try:
                settled.append({**self.apply(request, key=key, retries=0), "key": key})
            except WriteUnsettled:
                pass
        return settled

    def replace_session(self, token: str) -> list:
        """Swaps to a renewed token for the same principal and retries unsettled writes."""
        current = self._ensure_principal()
        following = self._describe(token)
        if current is None or following != current:
            raise ScopeConflict()
        self._token = token
        return self.retry_pending()

    # -- operations ---------------------------------------------------------------------------
    def operation(self, name: str, input: dict) -> Any:
        self._ensure_principal()
        self._ensure_contract()
        response = self._checked(self._post("/operation", {"operation": name, "input": copy.deepcopy(input)}), True)
        return response.get("output")

    def _managed(self, path: str, body: dict) -> dict:
        self._ensure_principal()
        self._ensure_contract()
        return self._checked(self._post(path, body), expect_contract=True)


class _Jobs:
    def __init__(self, client: Client):
        self._client = client

    def enqueue(self, operation: str, input: dict, key: str, delay_ms: int = 0) -> dict:
        return self._client._managed("/jobs/enqueue", {"operation": operation, "input": input, "key": key, "delayMs": delay_ms})["run"]

    def status(self, id: str) -> dict:
        return self._client._managed("/jobs/status", {"id": id})["run"]

    def cancel(self, id: str) -> dict:
        return self._client._managed("/jobs/cancel", {"id": id})["run"]

    def result(self, id: str) -> Any:
        return self._client._managed("/jobs/result", {"id": id})["output"]

    def wait(self, id: str, timeout: float = 30.0, interval: float = 0.2) -> dict:
        import time
        deadline = time.monotonic() + timeout
        while True:
            run = self.status(id)
            if run["state"] not in ("QUEUED", "RUNNING", "CANCEL_REQUESTED") or time.monotonic() >= deadline:
                return run
            time.sleep(interval)


class _Files:
    def __init__(self, client: Client):
        self._client = client

    def put(self, bucket: str, name: str, data: bytes, media_type: str) -> dict:
        body = {"bucket": bucket, "name": name, "mediaType": media_type, "base64": base64.b64encode(data).decode("ascii")}
        return self._client._managed("/files/put", body)["file"]

    def get(self, id: str) -> bytes:
        return base64.b64decode(self._client._managed("/files/get", {"id": id})["base64"])

    def delete(self, id: str) -> bool:
        return bool(self._client._managed("/files/delete", {"id": id}).get("deleted"))


class _Events:
    def __init__(self, client: Client):
        self._client = client

    def read(self, stream: str, after: str = "0", limit: int = 100) -> dict:
        response = self._client._managed("/events/read", {"stream": stream, "after": after, "limit": limit})
        return {"events": response["events"], "cursor": response["cursor"]}

    def watch(self, stream: str, after: str = "0", interval: float = 1.0) -> Iterator[dict]:
        import time
        cursor = after
        while True:
            page = self.read(stream, after=cursor)
            yield from page["events"]
            cursor = page["cursor"]
            if not page["events"]:
                time.sleep(interval)


def connect(base_url: str, token: Optional[str], contract_fingerprint: Optional[str] = None, **options: Any) -> Client:
    """Pin `contract_fingerprint` (from `aip service gen` output or a reviewed deployment) to detect drift;
    without it the client adopts the fingerprint reported by `/capabilities`."""
    return Client(base_url, token, contract_fingerprint, **options)

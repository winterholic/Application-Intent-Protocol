"""Official Python caller SDK for an AIP service.

The server stays the final authority: this client only expresses reads, transitions,
operations and durable jobs that the deployed contract exposes. It mirrors the TypeScript
transport rules for contract fingerprints, session scope and unsettled-write recovery.
Only the standard library is used so it runs in scripts, workers and other backends.
"""

from __future__ import annotations

import base64
import copy
import json
import re
import urllib.error
import urllib.request
import uuid
from typing import Any, Callable, Iterator, Optional

__all__ = ["connect", "Client", "AipError", "WriteUnsettled", "ScopeConflict"]
__version__ = "0.1.0"

_FINGERPRINT = re.compile(r"^[a-f0-9]{64}$")
_ACTOR_ID = re.compile(r"^(0|[1-9][0-9]*|-[1-9][0-9]*)$")
# A write whose commit could not be confirmed is resent with the same key and body.
_UNSETTLED = {"COMMIT_UNKNOWN", "INTERNAL"}
# Rejections before execution say nothing about whether an earlier attempt with the key committed.
_NOT_EXECUTED = {
    "TOKEN_EXPIRED", "UNAUTHENTICATED", "BAD_REQUEST", "PAYLOAD_TOO_LARGE", "NOT_FOUND", "CONTRACT_MISMATCH",
    "ORIGIN_NOT_ALLOWED", "REQUEST_TIMEOUT", "DB_UNAVAILABLE", "WORKER_BUSY", "DEADLINE_EXCEEDED", "CONFLICT",
}
_CONTRACT_PATHS = {"/read", "/apply", "/status", "/extension", "/operation"}
_MANAGED_PREFIXES = ("/jobs/", "/files/", "/events/read", "/messages/", "/rag/", "/objects/")


class AipError(Exception):
    """A typed server or protocol rejection. `code` is the stable AIP error code."""

    def __init__(self, code: str, message: Optional[str] = None):
        super().__init__(message or code)
        self.code = code


class WriteUnsettled(AipError):
    """The write may or may not have committed. Retry with `client.retry_pending()` or the same key."""

    def __init__(self, key: str, request: Any):
        super().__init__("WRITE_UNSETTLED", "쓰기 결과를 아직 확정하지 못함")
        self.key = key
        self.request = request


class ScopeConflict(AipError):
    def __init__(self) -> None:
        super().__init__("SCOPE_CONFLICT", "다른 사용자 세션으로 미확정 쓰기를 이관할 수 없음")


Transport = Callable[[str, dict, bytes, float], bytes]


def _urllib_transport(url: str, headers: dict, body: bytes, timeout: float) -> bytes:
    request = urllib.request.Request(url, data=body, headers=headers, method="POST")
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            return response.read()
    except urllib.error.HTTPError as error:
        # Webhook-style status codes still carry the JSON envelope.
        return error.read()


class Client:
    def __init__(self, base_url: str, token: Optional[str], contract_fingerprint: Optional[str] = None,
                 timeout: float = 30.0, transport: Optional[Transport] = None):
        if contract_fingerprint is not None and not _FINGERPRINT.match(contract_fingerprint):
            raise AipError("PROTOCOL_ERROR", "계약 식별값 형식 오류")
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
        try:
            return json.loads(raw)
        except (ValueError, TypeError):
            raise AipError("PROTOCOL_ERROR", "JSON 응답이 아님") from None

    def _checked(self, response: Any, expect_contract: bool) -> dict:
        if isinstance(response, dict) and response.get("ok") is False and isinstance(response.get("code"), str):
            raise AipError(response["code"], response.get("msg") if isinstance(response.get("msg"), str) else None)
        if not isinstance(response, dict) or response.get("ok") is not True:
            raise AipError("PROTOCOL_ERROR", "응답 형식 오류")
        if expect_contract and response.get("contractFingerprint") != self.contract_fingerprint:
            raise AipError("CONTRACT_MISMATCH", "고정한 계약과 서버 계약이 다름")
        return response

    def _describe(self, token: Optional[str]) -> Optional[str]:
        response = self._post("/session", {}, token=token, contract=False)
        if isinstance(response, dict) and response.get("ok") is False and isinstance(response.get("code"), str):
            raise AipError(response["code"])
        actor = (response.get("principal") or {}).get("actorId") if isinstance(response, dict) else None
        if response.get("ok") is not True or (token and not (isinstance(actor, str) and _ACTOR_ID.match(actor))):
            raise AipError("PROTOCOL_ERROR", "세션 응답 계약 오류")
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
            raise AipError("PROTOCOL_ERROR", "계약 식별값이 없음")
        if self._pinned and served != self.contract_fingerprint:
            raise AipError("CONTRACT_MISMATCH", "고정한 계약과 서버 계약이 다름")
        self.contract_fingerprint = served
        return response

    # -- data ---------------------------------------------------------------------------------
    def read(self, query: dict) -> list:
        self._ensure_principal()
        self._ensure_contract()
        response = self._checked(self._post("/read", {"query": copy.deepcopy(query)}), expect_contract=True)
        if not isinstance(response.get("rows"), list):
            raise AipError("PROTOCOL_ERROR", "읽기 응답 계약 오류")
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
            return {"ok": False, "code": "IDEMPOTENCY_MISMATCH"}
        self._pending[key] = snapshot
        for attempt in range(retries + 1):
            try:
                response = self._post("/apply", {"request": snapshot, "key": key})
            except (OSError, AipError):
                continue
            code = response.get("code") if isinstance(response, dict) else None
            if not isinstance(response, dict) or (response.get("ok") is not True and not isinstance(code, str)):
                continue
            if code in _UNSETTLED or (attempt > 0 and code in _NOT_EXECUTED):
                continue
            del self._pending[key]
            if attempt > 0:
                response = {**response, "recovered": True}
            return response
        raise WriteUnsettled(key, copy.deepcopy(snapshot))

    def status(self, request: dict, key: str) -> dict:
        """Committed result for a key and identical request, or `NOT_FOUND` if none is committed yet."""
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

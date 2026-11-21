"""Types of the `ctx` an AIP extension (Python) receives: `async def run(input, ctx)` in `<module>.py`.

They describe workers/worker.py as it is: argument order, optional arguments and what each call returns. Typing only; the worker
does not import this module. Use it in an extension without a runtime dependency:

    from typing import TYPE_CHECKING
    if TYPE_CHECKING:
        from aip_ctx import Ctx

    async def run(input: dict, ctx: "Ctx") -> dict: ...

This is the common surface. It does not know your `access` list: a call the server refuses for a missing grant (for example
`create.Todo` not in `access`) still type-checks and ends in `ctx.<op>.access_not_declared` at run time. Narrowing by grant is the
job of the generated SDK types, not of this file.
"""
from __future__ import annotations

from typing import Any, Mapping, Protocol, Sequence, TypedDict, Union

# The server's id wire decides the form: str on `decimal` (the service default) and `uuid`, int on the legacy wire.
EntityId = Union[str, int]


class ReadResult(TypedDict):
    rows: list[dict[str, Any]]


class ApplyResult(TypedDict):
    changed: list[EntityId]
    unchanged: list[EntityId]


class Data(Protocol):
    async def aggregate(self, res: str, name: str, inp: Mapping[str, Any]) -> Any:
        """A public aggregate of the caller's rows."""

    async def apply(
        self,
        res: str,
        name: str,
        ids: list[str | int] | list[str] | list[int],
        key: str | None = None,
        values: Mapping[str, Any] | None = None,
        expect: Mapping[str, Any] | None = None,
    ) -> ApplyResult:
        """A declared public transition on rows the caller can see (access `<Resource>.<transition>`).

        `ids` is a list (a bare `str` would be split into characters by worker.py, so it is not accepted).
        `key` names the call for you (1..100 characters, no control characters). `values` and `expect` are for a patch transition
        (the new values, and the values the rows must still hold; a mismatch ends in `WIRE.CONFLICT`). The transition's
        `input { ... }` slot (P0-B) is not forwarded by worker.py yet, so it is not declared here.
        """

    async def read(self, query: Mapping[str, Any]) -> ReadResult:
        """A public read of the caller's rows (access `read.<Resource>`). `query` is `{"read": ..., "select": [...], "filter": [...],
        "sort": [...], "limit": n}`; inside a write extension it sees the transaction's own writes."""

    async def create(self, res: str, values: Mapping[str, Any], key: str | None = None) -> EntityId | None:
        """One row of the resource's `expose create` as the caller (access `create.<Resource>`): the create's field list, `allow` and
        readable-reference rules are the client's. Returns the new row's id. `key` is only checked in a PostgreSQL write extension
        (the same key twice in one call makes two rows); a durable job journals it."""


class Mongo(Protocol):
    async def find(self, collection: str, filter: Mapping[str, Any] | None = None, projection: Mapping[str, Any] | None = None, limit: int | None = None) -> Any: ...
    async def find_one(self, collection: str, filter: Mapping[str, Any] | None = None, projection: Mapping[str, Any] | None = None) -> Any: ...
    async def count_documents(self, collection: str, filter: Mapping[str, Any] | None = None) -> int: ...
    async def insert_one(self, collection: str, document: Mapping[str, Any], compensate: bool = False, key: str | None = None) -> Any: ...
    async def update_one(self, collection: str, filter: Mapping[str, Any], update: Mapping[str, Any], upsert: bool = False, compensate: bool = False, key: str | None = None) -> Any: ...
    async def find_one_and_update(self, collection: str, filter: Mapping[str, Any], update: Mapping[str, Any], upsert: bool = False, compensate: bool = False, key: str | None = None) -> Any: ...
    async def delete_one(self, collection: str, filter: Mapping[str, Any], compensate: bool = False, key: str | None = None) -> int: ...


class Redis(Protocol):
    async def get(self, key: str) -> Any: ...
    async def hgetall(self, key: str) -> Any: ...
    async def exists(self, key: str) -> bool: ...
    async def mget(self, keys: Sequence[str]) -> Any: ...
    async def lrange(self, key: str, start: int = 0, stop: int = -1) -> list[Any]: ...
    async def replace_hash_ex(self, key: str, fields: Mapping[str, Any], ttl_seconds: int, effect_key: str | None = None) -> int: ...
    async def set_ex(self, key: str, value: Any, ttl_seconds: int, compensate: bool = False, effect_key: str | None = None) -> None: ...
    async def set_nx_ex(self, key: str, value: Any, ttl_seconds: int, compensate: bool = False, effect_key: str | None = None) -> bool: ...
    async def delete(self, key: str, compensate: bool = False, effect_key: str | None = None) -> int: ...
    async def getdel(self, key: str, compensate: bool = False, effect_key: str | None = None) -> Any: ...
    async def incr_expire(self, key: str, ttl_seconds: int, effect_key: str | None = None) -> Any: ...
    async def expire(self, key: str, ttl_seconds: int, effect_key: str | None = None) -> bool: ...
    async def push_trim_expire(self, key: str, value: Any, max_len: int, ttl_seconds: int, effect_key: str | None = None) -> None: ...


class Http(Protocol):
    async def call(self, name: str, inp: Any, key: str | None = None) -> Any: ...


class Webhooks(Protocol):
    async def deliver(self, name: str, url: str, payload: Any, key: str) -> Any: ...


class Files(Protocol):
    async def put(self, bucket: str, inp: Mapping[str, Any], key: str | None = None) -> Any: ...
    async def get(self, bucket: str, id: str) -> Any: ...


class Events(Protocol):
    async def publish(self, stream: str, key: str, payload: Any) -> Any: ...


class Messages(Protocol):
    async def publish(self, queue: str, payload: Any, key: str) -> Any: ...


class Jobs(Protocol):
    async def run(self, name: str, inp: Any, key: str) -> Any: ...


class AipError(Exception):
    """What `raise ctx.reject(...)` and `raise ctx.retry_later(...)` build: an error the worker reports with its code."""

    code: str
    after_ms: int | None
    app_code: str | None
    message: str | None


class Ctx(Protocol):
    data: Data
    mongo: Mongo
    redis: Redis
    http: Http
    webhooks: Webhooks
    files: Files
    events: Events
    messages: Messages
    jobs: Jobs
    # Durable job only: {"id": ..., "attempt": n} (attempt from 1). None outside a job.
    job: Mapping[str, Any] | None

    def reject(self, app_code: str, message: str | None = None) -> AipError:
        """`raise ctx.reject("INSUFFICIENT_STOCK", "재고가 부족합니다")`: a business refusal. The server undoes everything written and the
        caller gets `WIRE.EXTENSION_REJECTED` with this code and text. The code is upper-case letters, digits and `_` (starts with a
        letter, at most 48 bytes); the text is at most 200 characters without control characters. The server checks both."""

    def retry_later(self, reason: str | None = None, after_ms: int | None = None) -> AipError:
        """Durable job only: `raise ctx.retry_later("why", after_ms=...)` runs the same job again after the backoff."""

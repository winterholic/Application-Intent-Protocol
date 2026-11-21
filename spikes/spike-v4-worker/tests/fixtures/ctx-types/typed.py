# An extension written against workers/aip_ctx.py: it type-checks (mypy) and it runs in the real worker.py, so the declared
# argument order and result shapes are held to what the worker sends and returns (tests/worker_ctx_types.rs).
from typing import TYPE_CHECKING, Any

if TYPE_CHECKING:
    from aip_ctx import Ctx


async def data(input: dict[str, Any], ctx: "Ctx") -> dict[str, Any]:
    total = await ctx.data.aggregate("Order", "revenue", {"month": "2026-11"})
    page = await ctx.data.read(
        {
            "read": "Todo",
            "select": ["id", "title", {"owner": {"select": ["id"]}}],
            "filter": [{"field": "id", "op": "eq", "value": "10"}],
            "sort": [{"field": "id", "dir": "asc"}],
            "limit": 5,
        }
    )
    mixed: list[str | int] = [10, "11"]
    done = await ctx.data.apply("Todo", "complete", mixed, "complete-1", {"note": "x"}, {"note": None})
    bare = await ctx.data.apply("Todo", "complete", ["10"])
    await ctx.data.apply("Todo", "take", ["10"], input={"qty": 3})
    made = await ctx.data.create("Todo", {"owner": input["owner"], "title": "next", "done": False}, "todo-1")
    no_key = await ctx.data.create("Todo", {"owner": input["owner"], "title": "again", "done": False})
    # The declared result shapes, held in place by assignments mypy would refuse if a declaration changed.
    made_id: str | int | None = made
    changed_ids: list[str | int] = done["changed"]
    rows: list[dict[str, Any]] = page["rows"]
    _held = (made_id, changed_ids, rows)  # stdout is the worker protocol: never print
    return {"sum": total, "rows": len(page["rows"]), "changed": done["changed"], "unchanged": bare["unchanged"], "made": made, "noKey": no_key}


async def external(input: dict[str, Any], ctx: "Ctx") -> dict[str, Any]:
    found = await ctx.mongo.find("events", {"a": 1}, {"_id": 0}, 3)
    inserted = await ctx.mongo.insert_one("events", {"a": 1}, True)
    kept = await ctx.mongo.delete_one("events", {"a": 1}, compensate=True, key="d-1")
    flag = await ctx.redis.set_nx_ex("lock:1", "v", 30, True)
    items = await ctx.redis.lrange("list:1")
    return {"found": found, "inserted": inserted, "kept": kept, "flag": flag, "items": items}


async def refuses(input: dict[str, Any], ctx: "Ctx") -> dict[str, Any]:
    raise ctx.reject("INSUFFICIENT_STOCK", "재고가 부족합니다")

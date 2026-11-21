# Every line that ends in an ERR comment must be refused by mypy, and no other line may be (tests/worker_ctx_types.rs). Never run.
from aip_ctx import Ctx


async def wrong(ctx: Ctx) -> None:
    await ctx.data.apply("Todo", "complete", "10")  # ERR ids must be a list, not a str
    await ctx.data.apply("Todo", "complete", [{"id": 10}])  # ERR an id is a str or int
    await ctx.data.apply("Todo", "complete", ["10"], 7)  # ERR key is a str
    await ctx.data.create("Todo", "title")  # ERR values is a mapping
    await ctx.data.create("Todo")  # ERR values is required
    await ctx.data.read("Todo")  # ERR read takes the query mapping
    await ctx.data.crete("Todo", {})  # ERR a misspelled method
    await ctx.dat.create("Todo", {})  # ERR a misspelled namespace
    await ctx.mongo.delete_one("events", {}, compensate="yes")  # ERR compensate is a bool
    raise ctx.reject(409, "x")  # ERR the business code is a str


async def typed_results(ctx: Ctx) -> None:
    made = await ctx.data.create("Todo", {})
    as_bool: bool = made  # ERR the id is a str, an int or None
    strict: str | int = made  # ERR create may return None
    done = await ctx.data.apply("Todo", "complete", ["10"])
    done["missing"]  # ERR no such key
    page = await ctx.data.read({"read": "Todo", "select": ["id"]})
    page["rows"] = 1  # ERR rows is a list
    print(as_bool, strict)

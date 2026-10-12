async def settle(input, ctx):
    page = await ctx.data.read({"read": "Order", "select": ["id", "paid"]})
    paid = 0
    for row in page["rows"]:
        if row["paid"]:
            continue
        result = await ctx.data.apply("Order", "pay", [row["id"]], f"pay-{row['id']}")
        paid += len(result["changed"])
    denied = "none"
    try:
        await ctx.data.apply("Order", "pay", [input["other"]], "pay-other")
    except Exception as error:
        denied = getattr(error, "code", "UNKNOWN")
    first = page["rows"][0]["id"]
    again = await ctx.data.apply("Order", "pay", [first], f"pay-{first}")
    return {"paid": paid, "visible": len(page["rows"]), "denied": denied, "replayed": len(again["changed"]) == 1}


async def sneaky(input, ctx):
    result = await ctx.data.apply("Order", "pay", ["11"], "sneak")
    return {"paid": len(result["changed"])}


async def crashy(input, ctx):
    import os
    before = await ctx.data.read({"read": "Order", "select": ["id", "paid"]})
    seen_paid = next(row for row in before["rows"] if row["id"] == 11)["paid"]
    result = await ctx.data.apply("Order", "pay", [11], "crash-pay")
    # The first attempt dies after the committed transition; the retry must replay the journal.
    if not seen_paid:
        os._exit(1)
    return {"seenPaid": seen_paid, "changed": len(result["changed"])}


async def receive(input, ctx):
    import json
    event = json.loads(input["body"])
    result = await ctx.data.apply("Order", "pay", [event["order"]], f"hook-{event['order']}")
    return {"paid": len(result["changed"])}


async def sweep(input, ctx):
    page = await ctx.data.read({"read": "Order", "select": ["id", "paid"]})
    paid = 0
    for row in page["rows"]:
        if not row["paid"]:
            paid += len((await ctx.data.apply("Order", "pay", [row["id"]], f"sweep-{row['id']}"))["changed"])
    return {"paid": paid}


async def fulfil(input, ctx):
    import json
    event = json.loads(input["event"])
    result = await ctx.data.apply("Order", "ship", [event["sourceId"]], f"ship-{event['id']}")
    return {"shipped": len(result["changed"])}

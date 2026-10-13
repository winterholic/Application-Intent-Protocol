# A subscription is disabled after this many deliveries in a row failed. The count lives in the subscription row, so the
# policy is the application's: change the number, count other failures, or notify the owner when it disables.
MAX_CONSECUTIVE_FAILURES = 3


async def fanout(input, ctx):
    event = {"type": "order.paid", "event": input["event"]}
    page = await ctx.data.read({"read": "Subscription", "select": ["id", "url", "active", "failures"]})
    delivered = failed = disabled = 0
    for subscription in [row for row in page["rows"] if row["active"]]:
        key = f"{input['event']}-{subscription['id']}"
        try:
            await ctx.webhooks.deliver("subscribers", subscription["url"], event, key)
            delivered += 1
            # One success ends the streak.
            if subscription["failures"] > 0:
                await ctx.data.apply("Subscription", "recovered", [subscription["id"]], f"recovered-{key}")
        except Exception as error:  # noqa: BLE001
            code = getattr(error, "code", "UNKNOWN")
            if code not in ("WIRE.DELIVERY_FAILED", "WIRE.DESTINATION_REJECTED"):
                raise
            failed += 1
            # A rejected destination (a private address, a malformed URL) never becomes valid by waiting; a failed delivery may.
            hopeless = code == "WIRE.DESTINATION_REJECTED" or subscription["failures"] + 1 >= MAX_CONSECUTIVE_FAILURES
            await ctx.data.apply("Subscription", "failed", [subscription["id"]], f"failed-{key}")
            if hopeless:
                await ctx.data.apply("Subscription", "disable", [subscription["id"]], f"disable-{key}")
                disabled += 1
    return {"delivered": delivered, "failed": failed, "disabled": disabled}

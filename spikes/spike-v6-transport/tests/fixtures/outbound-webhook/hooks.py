async def notify(input, ctx):
    event = {"type": "order.paid", "order": 11}
    sent = await ctx.webhooks.deliver("subscribers", input["url"], event, "order-11-paid")
    again = await ctx.webhooks.deliver("subscribers", input["url"], event, "order-11-paid")

    async def code(url, key):
        try:
            await ctx.webhooks.deliver("subscribers", url, event, key)
            return "sent"
        except Exception as error:
            return getattr(error, "code", "UNKNOWN")

    return {
        "status": sent["status"],
        "id": sent["id"],
        "same": again["id"] == sent["id"],
        "privateCode": await code(input["private"], "private"),
        "failingCode": await code(input["failing"], "failing"),
    }

import json


async def consume(input, ctx):
    message = json.loads(input["message"])
    return {"order": message["payload"]["order"], "deliveries": message["deliveries"]}

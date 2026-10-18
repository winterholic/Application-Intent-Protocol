# 읽기 확장의 ctx.data.read가 선언된 resource에서만 host로 가는지 확인하는 확장.
async def readRecruitment(input, ctx):
    r = await ctx.data.read({"read": "Recruitment", "select": ["id"], "limit": 1})
    return {"approvedApplicants": len(r["rows"])}


async def aggregateOnly(input, ctx):
    r = await ctx.data.aggregate("Apply", "approvedCount", {"clubId": input["clubId"]})
    return {"approvedApplicants": r["value"]}


async def applyThroughRead(input, ctx):
    await ctx.call("apply", "Recruitment.close", {"ids": ["1"]})
    return {"approvedApplicants": 0}

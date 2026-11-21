"""주문 확정: 여러 상품의 재고를 한 트랜잭션에서 차감하고 합계를 계산한다.
선언 전이는 `stock = stock - 고정값`만 표현하므로 가변 수량 차감과 합계 계산은 확장에서 한다."""
from decimal import Decimal


async def place(input, ctx):
    order_id = input["order"]
    lines = await ctx.data.read({"read": "OrderLine", "select": ["id", "product", "qty", "unitPrice"],
                                 "filter": [{"field": "order", "op": "eq", "value": order_id}]})
    if not lines["rows"]:
        raise ValueError("empty order")
    need = {}
    total = Decimal("0")
    for row in lines["rows"]:
        need[row["product"]] = need.get(row["product"], 0) + row["qty"]
        total += Decimal(row["unitPrice"]) * row["qty"]
    products = await ctx.data.read({"read": "Product", "select": ["id", "stock"],
                                    "filter": [{"field": "id", "op": "in", "value": list(need)}]})
    stock = {p["id"]: p["stock"] for p in products["rows"]}
    for pid, qty in need.items():
        if stock.get(pid, 0) < qty:
            raise ValueError("insufficient stock")
    for pid, qty in need.items():
        await ctx.data.apply("Product", "take", [pid], None, {"stock": stock[pid] - qty}, {"stock": stock[pid]})
    await ctx.data.apply("Order", "confirm", [order_id], None, {"total": str(total)})
    return {"order": order_id, "total": str(total), "lines": len(lines["rows"])}

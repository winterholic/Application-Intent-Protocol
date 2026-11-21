"""StockDesk 소비자 e2e: Python SDK(wheel 설치)로 실제 서버에 호출한다."""
import os
import sys
import time
from aip_sdk import connect, AipError
from stock_binding import CONTRACT_FINGERPRINT, apply_product_restock, ProductId

URL = os.environ.get("AIP_URL", "http://127.0.0.1:18082")
RUN = os.environ.get("RUN_ID", str(int(time.time())))
mgr1 = connect(URL, os.environ["TOKEN_MGR1"], CONTRACT_FINGERPRINT)
clerk = connect(URL, os.environ["TOKEN_CLERK1"], CONTRACT_FINGERPRINT)
mgr2 = connect(URL, os.environ["TOKEN_MGR2"], CONTRACT_FINGERPRINT)
anon = connect(URL, None, CONTRACT_FINGERPRINT)
n = 0


def step(name, extra=None):
    global n
    n += 1
    print(f"PASS {n:02d} {name}" + (f" {extra}" if extra is not None else ""))


def read(c, q):
    return c.read(q)


# 인증 조회 + 테넌트 격리
assert mgr1.session() == {"actorId": "1"}
step("session mgr1", mgr1.session())
tenants = read(mgr1, {"read": "Tenant", "select": ["id", "name"]})
assert [t["name"] for t in tenants] == ["Gangnam Store"], tenants
step("tenant isolation: only own tenant visible", tenants)
staff = read(mgr1, {"read": "Staff", "select": ["id", "name", "role"]})
assert len(staff) == 2
step("staff of own tenant only", staff)

# 생성: 상품 (매니저만)
sku = f"SKU-{RUN}"
made = mgr1.bundle([
    {"create": "Product", "values": {"tenant": "1", "sku": sku + "-A", "name": "Widget", "unitPrice": "10.50", "stock": 100, "reorderLevel": 10}},
    {"create": "Product", "values": {"tenant": "1", "sku": sku + "-B", "name": "Gadget", "unitPrice": "3.25", "stock": 5, "reorderLevel": 2}},
], key=f"prod-{RUN}")
assert made["ok"], made
pa, pb = [c["id"] for c in made["created"]]
step("manager creates 2 products atomically", {"A": pa, "B": pb})
denied = clerk.bundle([{"create": "Product", "values": {"tenant": "1", "sku": sku + "-C", "name": "x", "unitPrice": "1.00", "stock": 1, "reorderLevel": 0}}], key=f"prodc-{RUN}")
assert denied["ok"] is False
step("clerk create product -> denied", denied["code"])
dup = mgr1.bundle([{"create": "Product", "values": {"tenant": "1", "sku": sku + "-A", "name": "dup", "unitPrice": "1.00", "stock": 1, "reorderLevel": 0}}], key=f"dup-{RUN}")
assert dup["ok"] is False and dup["code"] == "WIRE.ALREADY_EXISTS"
step("duplicate sku in tenant -> WIRE.ALREADY_EXISTS", dup["code"])
cross = mgr1.bundle([{"create": "Product", "values": {"tenant": "2", "sku": sku + "-X", "name": "x", "unitPrice": "1.00", "stock": 1, "reorderLevel": 0}}], key=f"cross-{RUN}")
assert cross["ok"] is False
step("create in another tenant -> denied", cross["code"])

# 주문 + 줄 (줄 단가는 상품 단가와 같아야 한다는 정의 규칙)
o = clerk.bundle([{"create": "Order", "values": {"tenant": "1", "placedBy": "2", "number": f"O-{RUN}-1", "status": "OPEN", "total": "0.00"}}], key=f"ord-{RUN}")
assert o["ok"], o
oid = o["created"][0]["id"]
bad = clerk.bundle([{"create": "OrderLine", "values": {"order": oid, "product": pa, "qty": 2, "unitPrice": "0.01"}}], key=f"badline-{RUN}")
assert bad["ok"] is False
step("order line with tampered price -> denied", bad["code"])
lines = clerk.bundle([
    {"create": "OrderLine", "values": {"order": oid, "product": pa, "qty": 4, "unitPrice": "10.50"}},
    {"create": "OrderLine", "values": {"order": oid, "product": pb, "qty": 2, "unitPrice": "3.25"}},
    {"create": "OrderLine", "values": {"order": oid, "product": pa, "qty": 1, "unitPrice": "10.50"}},
], key=f"lines-{RUN}")
assert lines["ok"], lines
step("clerk creates order + 3 lines", {"order": oid})

# 확장: Python write extension
placed = clerk.apply({"extension": "Order.place", "input": {"order": oid}}, key=f"place-{RUN}")
print(placed)
assert placed["ok"] and placed["output"]["total"] == "59.00", placed
step("python write extension Order.place", placed["output"])
prods = {p["sku"]: p for p in read(mgr1, {"read": "Product", "select": ["id", "sku", "stock"], "filter": [{"field": "sku", "op": "prefix", "value": sku}]})}
assert prods[sku + "-A"]["stock"] == 95 and prods[sku + "-B"]["stock"] == 3, prods
step("stock decremented atomically", {k: v["stock"] for k, v in prods.items()})
replay = clerk.apply({"extension": "Order.place", "input": {"order": oid}}, key=f"place-{RUN}")
assert replay.get("replayed") is True
prods2 = {p["sku"]: p["stock"] for p in read(mgr1, {"read": "Product", "select": ["sku", "stock"], "filter": [{"field": "sku", "op": "prefix", "value": sku}]})}
assert prods2 == {sku + "-A": 95, sku + "-B": 3}, prods2
step("same key replay does not double-decrement", {"replayed": True, "stock": prods2})

# 재고 부족: 전체 롤백
o2 = clerk.bundle([{"create": "Order", "values": {"tenant": "1", "placedBy": "2", "number": f"O-{RUN}-2", "status": "OPEN", "total": "0.00"}}], key=f"ord2-{RUN}")
oid2 = o2["created"][0]["id"]
clerk.bundle([{"create": "OrderLine", "values": {"order": oid2, "product": pa, "qty": 1, "unitPrice": "10.50"}},
              {"create": "OrderLine", "values": {"order": oid2, "product": pb, "qty": 99, "unitPrice": "3.25"}}], key=f"lines2-{RUN}")
short = clerk.apply({"extension": "Order.place", "input": {"order": oid2}}, key=f"place2-{RUN}")
print(short)
assert short["ok"] is False
st = {p["sku"]: p["stock"] for p in read(mgr1, {"read": "Product", "select": ["sku", "stock"], "filter": [{"field": "sku", "op": "prefix", "value": sku}]})}
assert st == {sku + "-A": 95, sku + "-B": 3}
step("insufficient stock -> rejected, nothing changed", {"code": short["code"], "msg": short.get("msg")})

# 관계 + 조건 조회
orders = read(mgr1, {"read": "Order", "select": ["id", "number", "status", "total", "lineCount", {"placedBy": {"select": ["id", "name"]}}, {"OrderLine": {"select": ["id", "qty", "unitPrice"], "limit": 10}}],
                      "filter": [{"field": "status", "op": "eq", "value": "CONFIRMED"}], "sort": [{"field": "id", "dir": "desc"}]})
top = [r for r in orders if r["id"] == oid][0]
assert top["lineCount"] == 3 and top["placedBy"]["name"] == "Joo" and len(top["OrderLine"]) == 3 and top["total"] == "59.00", top
step("relation read: order + placedBy + lines + lineCount aggregate", {"id": top["id"], "lineCount": top["lineCount"]})
low = read(mgr1, {"read": "Product", "select": ["sku", "stock", "reorderLevel"], "filter": [{"field": "stock", "op": "lte", "value": 3}, {"field": "sku", "op": "prefix", "value": sku}], "sort": [{"field": "stock", "dir": "asc"}]})
assert [p["sku"] for p in low] == [sku + "-B"]
step("filter+sort: low-stock products", low)

# 권한 없는 요청
assert read(mgr2, {"read": "Order", "select": ["id"]}) == []
step("other tenant manager sees no orders")
x = mgr2.apply({"apply": "Order.cancel", "target": {"ids": [oid2]}}, key=f"x-{RUN}")
assert x["ok"] is False
step("other tenant cancel -> denied", x["code"])
r = clerk.apply({"apply": "Product.restock", "target": {"ids": [pa]}, "values": {"stock": 500}, "expect": {"stock": 95}}, key=f"cr-{RUN}")
assert r["ok"] is False
step("clerk restock -> denied", r["code"])
r = mgr1.apply({"apply": "Product.take", "target": {"ids": [pa]}, "values": {"stock": 0}, "expect": {"stock": 95}}, key=f"take-{RUN}")
assert r["ok"] is False
step("internal transition Product.take not callable publicly", r["code"])
assert read(anon, {"read": "Product", "select": ["id"]}) == []
step("anonymous read -> no rows")
try:
    read(mgr1, {"read": "Product", "select": ["id", "cost"]})
    raise SystemExit("unexposed field accepted")
except AipError as e:
    step("unexposed field -> rejected", e.code)

# 수정 (generated helper) + 낙관적 잠금
ok = apply_product_restock(mgr1, {"ids": [ProductId(pa)]}, {"stock": 195}, {"stock": 95}, key=f"restock-{RUN}")
assert ok["ok"], ok
stale = apply_product_restock(mgr1, {"ids": [ProductId(pa)]}, {"stock": 300}, {"stock": 95}, key=f"restock2-{RUN}")
assert stale["ok"] is False and stale["code"] == "WIRE.CONFLICT"
step("restock via generated helper; stale expect -> WIRE.CONFLICT", stale["code"])
c = mgr1.apply({"apply": "Order.cancel", "target": {"ids": [oid2]}}, key=f"cancel-{RUN}")
assert c["ok"]
step("manager cancels open order", c)
print("ALL PASS")

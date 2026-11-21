import os, sys, json, time, warnings
sys.path.insert(0, '/Users/winterholic/development/projects/aip/aip-harden-contract-20261121/product/python')
import aip_sdk
from aip_sdk import connect, AipError, WriteUnsettled, created
import binding
TOKEN = open('token.txt').read().strip()
URL = 'http://127.0.0.1:18471'
def log(l, x): print(l, json.dumps(x, default=str, ensure_ascii=False))
print('exports', sorted(n for n in dir(aip_sdk) if not n.startswith('_')))
c = connect(URL, TOKEN, binding.CONTRACT_FINGERPRINT)
log('session', c.session())
t = str(int(time.time()))
r = c.bundle([{"create": "Order", "values": {"owner": "42", "total": "3.00", "status": "open"}},
              {"create": "OrderLine", "values": {"product": "2", "qty": 1, "amount": "2.50"}, "refs": {"order": created(0)}},
              {"apply": "Product.take", "target": {"ids": ["2"]}, "input": {"qty": 1}}], key="py-b1-" + t)
log('bundle refs created(0)', r)
r = c.bundle([{"create": "Order", "values": {"owner": "42", "total": "1.00", "status": "open"}},
              {"apply": "Order.confirm", "target": created(0)}], key="py-b2-" + t)
log('bundle target created(0)', r)
r = c.create("Note", {"owner": "42", "text": "x"}, key="py-c1-" + t)
log('client.create', r); log('created id', r["created"][0]["id"])
r = binding.create_note(c, {"owner": binding.MemberId("42")}, key="py-c2-" + t)
log('create_note helper', r)
r = c.apply({"apply": "Product.take", "target": {"ids": ["1"]}, "input": {"qty": 1000}}, key="py-a1-" + t)
log('apply reject result', r)
try:
    c.read({"read": "Product", "select": ["nope"]})
except AipError as e:
    log('read reject raises', {"type": type(e).__name__, "code": e.code, "retry": getattr(e, 'retry', None), "request_id": getattr(e, 'request_id', None), "attrs": [a for a in dir(e) if not a.startswith('_')][:20]})
r = binding.apply_product_restock(c, {"ids": [binding.ProductId("2")]}, input={"amount": "0.25"}, key="py-r1-" + t)
log('restock helper', r)
r = c.apply({"apply": "Product.restock", "target": {"ids": ["2"]}, "input": {"amount": 1.25}}, key="py-r2-" + t)
log('restock decimal as float', r)
for tok in ("", "  "):
    try: connect(URL, tok, binding.CONTRACT_FINGERPRINT).session()
    except AipError as e: log('empty token', {"code": e.code})
a = connect(URL, None, binding.CONTRACT_FINGERPRINT); log('anon', a.session())
with warnings.catch_warnings(record=True) as w:
    warnings.simplefilter('always')
    d = c.discover() if hasattr(c, 'discover') else None
    log('discover', {"d": d, "warnings": [str(x.message) for x in w]})
bad = connect(URL, TOKEN, "0"*64)
try: log('bad fp', bad.read({"read": "Product", "select": ["id"]}))
except AipError as e: log('bad fp raises', {"code": e.code, "retry": getattr(e,'retry',None)})
r = c.compose("Order", ["1"], [{"transition": "confirm"}], key="py-cp-" + t)
log('compose', r)

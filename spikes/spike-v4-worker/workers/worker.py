# AIP 확장 worker(Python). 서버와 stdin/stdout JSON 줄로만 통신한다. DB 연결 정보는 받지 않는다.
import asyncio, importlib.util, inspect, json, os, sys

MAX_FRAME_BYTES = 1024 * 1024

ext_dir = sys.argv[1]
pending = {}
seq = 0


def send(m):
    sys.stdout.write(json.dumps(m, ensure_ascii=False) + "\n")
    sys.stdout.flush()


class AipError(Exception):
    def __init__(self, code, after_ms=None):
        super().__init__(code)
        self.code = code
        self.after_ms = after_ms


class Data:
    def __init__(self, ctx):
        self.ctx = ctx

    async def aggregate(self, res, name, inp):
        return await self.ctx.call("aggregate", f"{res}.{name}", inp)

    async def apply(self, res, name, ids, key=None):
        inp = {"ids": [str(i) for i in ids]}
        if key is not None:
            inp["key"] = key
        return await self.ctx.call("apply", f"{res}.{name}", inp)

    # durable job 전용: 공개된 create(expose create)로 새 행 하나를 만들고 id를 돌려준다. key는 effect 장부에 남는다.
    async def create(self, res, values, key=None):
        out = await self.ctx.call("create", res, {"values": values, "key": key})
        created = out.get("created") or []
        return created[0].get("id") if created else None

    # durable job 전용: 공개 read 요청을 job 주체의 정책으로 실행한다.
    async def read(self, query):
        return await self.ctx.call("read", query.get("read") if isinstance(query, dict) else None, query)


class Http:
    def __init__(self, ctx): self.ctx = ctx
    async def call(self, name, inp, key=None): return await self.ctx.call("http", name, {"input": inp, "key": key})

# durable job 전용: 업무 데이터의 구독자 URL로 서명한 이벤트를 보낸다(access http.<name>, 서버 설정 outbound_webhooks).
class Webhooks:
    def __init__(self, ctx): self.ctx = ctx
    async def deliver(self, name, url, payload, key): return await self.ctx.call("http", name, {"input": payload, "key": key, "url": url})

class Files:
    def __init__(self, ctx): self.ctx = ctx
    async def put(self, bucket, inp, key=None): return await self.ctx.call("files", bucket, {**inp, "action": "put", "key": key})
    async def get(self, bucket, id): return await self.ctx.call("files", bucket, {"action": "get", "id": id})

class Events:
    def __init__(self, ctx): self.ctx = ctx
    async def publish(self, stream, key, payload): return await self.ctx.call("events", stream, {"key": key, "payload": payload})

class Messages:
    def __init__(self, ctx): self.ctx = ctx
    async def publish(self, queue, payload, key): return await self.ctx.call("messages", queue, {"payload": payload, "key": key})

class Jobs:
    def __init__(self, ctx): self.ctx = ctx
    async def run(self, name, inp, key): return await self.ctx.call("jobs", name, {"input": inp, "key": key})

def _flag(inp, compensate, key=None):
    # A durable job names every write with a stable `key`, which the server journals.
    if compensate:
        inp["compensate"] = True
    if key is not None:
        inp["key"] = key
    return inp

class Mongo:
    def __init__(self, ctx): self.ctx = ctx
    async def find(self, collection, filter=None, projection=None, limit=None):
        inp = {"command": "find", "filter": filter or {}}
        if projection is not None: inp["projection"] = projection
        if limit is not None: inp["limit"] = limit
        res = await self.ctx.call("mongo", collection, inp)
        return res.get("docs", []) if isinstance(res, dict) else res
    async def find_one(self, collection, filter=None, projection=None):
        inp = {"command": "find_one", "filter": filter or {}}
        if projection is not None: inp["projection"] = projection
        res = await self.ctx.call("mongo", collection, inp)
        return res.get("doc") if isinstance(res, dict) else res
    async def count_documents(self, collection, filter=None):
        res = await self.ctx.call("mongo", collection, {"command": "count_documents", "filter": filter or {}})
        return res.get("count", 0) if isinstance(res, dict) else res
    # Writes (write extensions and durable jobs, `access mongo.<collection>`; a job passes key=... and has no saga). compensate=True keeps a way back: if the relational commit does not
    # happen the server undoes the write (insert: delete; upsert: delete or restore the old document; delete: insert it back).
    async def insert_one(self, collection, document, compensate=False, key=None):
        res = await self.ctx.call("mongo", collection, _flag({"command": "insert_one", "document": document}, compensate, key))
        return res.get("inserted_id")
    async def update_one(self, collection, filter, update, upsert=False, compensate=False, key=None):
        return await self.ctx.call("mongo", collection, _flag({"command": "update_one", "filter": filter, "update": update, "upsert": upsert}, compensate, key))
    async def find_one_and_update(self, collection, filter, update, upsert=False, compensate=False, key=None):
        res = await self.ctx.call("mongo", collection, _flag({"command": "find_one_and_update", "filter": filter, "update": update, "upsert": upsert}, compensate, key))
        return res.get("doc")
    async def delete_one(self, collection, filter, compensate=False, key=None):
        res = await self.ctx.call("mongo", collection, _flag({"command": "delete_one", "filter": filter}, compensate, key))
        return res.get("deleted", 0)

class Redis:
    def __init__(self, ctx): self.ctx = ctx
    async def get(self, key):
        res = await self.ctx.call("redis", key, {"command": "get"})
        return res.get("value") if isinstance(res, dict) else res
    async def hgetall(self, key):
        res = await self.ctx.call("redis", key, {"command": "hgetall"})
        return res.get("value") if isinstance(res, dict) else res
    async def exists(self, key):
        res = await self.ctx.call("redis", key, {"command": "exists"})
        return res.get("exists") if isinstance(res, dict) else False
    async def mget(self, keys):
        return await self.ctx.call("redis", keys[0] if keys else "", {"command": "mget", "keys": keys})
    async def lrange(self, key, start=0, stop=-1):
        res = await self.ctx.call("redis", key, {"command": "lrange", "start": start, "stop": stop})
        return res.get("values", [])
    async def replace_hash_ex(self, key, fields, ttl_seconds, effect_key=None):
        res = await self.ctx.call("redis", key, _flag({"command": "replace_hash_ex", "fields": fields, "ttl_seconds": ttl_seconds}, False, effect_key))
        return res.get("replaced", 0)
    # Writes (write extensions and durable jobs, `access redis "<key or prefix:*>"`; a job passes effect_key=...; replace_hash_ex and the TTL helpers take effect_key). Every key lives at most ttl_seconds (1 .. 30 days).
    async def set_ex(self, key, value, ttl_seconds, compensate=False, effect_key=None):
        await self.ctx.call("redis", key, _flag({"command": "set_ex", "value": value, "ttl_seconds": ttl_seconds}, compensate, effect_key))
    async def set_nx_ex(self, key, value, ttl_seconds, compensate=False, effect_key=None):
        res = await self.ctx.call("redis", key, _flag({"command": "set_nx_ex", "value": value, "ttl_seconds": ttl_seconds}, compensate, effect_key))
        return res.get("set", False)
    async def delete(self, key, compensate=False, effect_key=None):
        res = await self.ctx.call("redis", key, _flag({"command": "del"}, compensate, effect_key))
        return res.get("deleted", 0)
    async def getdel(self, key, compensate=False, effect_key=None):
        res = await self.ctx.call("redis", key, _flag({"command": "getdel"}, compensate, effect_key))
        return res.get("value")
    async def incr_expire(self, key, ttl_seconds, effect_key=None):
        res = await self.ctx.call("redis", key, _flag({"command": "incr_expire", "ttl_seconds": ttl_seconds}, False, effect_key))
        return res.get("value")
    async def expire(self, key, ttl_seconds, effect_key=None):
        res = await self.ctx.call("redis", key, _flag({"command": "expire", "ttl_seconds": ttl_seconds}, False, effect_key))
        return res.get("applied", False)
    async def push_trim_expire(self, key, value, max_len, ttl_seconds, effect_key=None):
        await self.ctx.call("redis", key, _flag({"command": "push_trim_expire", "value": value, "max_len": max_len, "ttl_seconds": ttl_seconds}, False, effect_key))

class Ctx:
    def __init__(self, token, job=None):
        self.token = token
        # durable job 전용: 실행 id와 몇 번째 시도인지(1부터). job이 아니면 None이다.
        self.job = job
        self.data = Data(self)
        self.http = Http(self)
        self.webhooks = Webhooks(self)
        self.files = Files(self)
        self.events = Events(self)
        self.jobs = Jobs(self)
        self.messages = Messages(self)
        self.mongo = Mongo(self)
        self.redis = Redis(self)

    # durable job 전용: `raise ctx.retry_later("이유")`로 같은 job을 backoff 뒤 다시 시도하게 한다.
    # `after_ms`를 주면 기본 backoff 대신 그 시간(서버가 1초~1시간으로 맞춘다) 뒤에 다시 시도한다.
    def retry_later(self, reason=None, after_ms=None):
        return AipError("WIRE.RETRY_LATER", after_ms)

    async def call(self, op, name, inp, **extra):
        global seq
        seq += 1
        fut = asyncio.get_running_loop().create_future()
        pending[seq] = fut
        send({"type": "call", "call": seq, "token": self.token, "op": op, "name": name, "input": inp, **extra})
        return await fut


modules = {}


def load(mod):
    # Node의 import처럼 worker 수명 동안 한 번만 실행한다. 모듈 전역 상태는 호출 사이에 남는다.
    if mod not in modules:
        spec = importlib.util.spec_from_file_location(mod, os.path.join(ext_dir, f"{mod}.py"))
        m = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(m)
        modules[mod] = m
    return modules[mod]


async def run_invoke(m):
    mod, fn = m["impl"].split(".")
    try:
        out = getattr(load(mod), fn)(m["input"], Ctx(m["token"], m.get("job")))
        if inspect.isawaitable(out):
            out = await out
        send({"type": "done", "invoke": m["invoke"], "output": out})
    except AipError as e:
        error = {"code": e.code, "message": e.code}
        if e.code == "WIRE.RETRY_LATER" and isinstance(e.after_ms, int) and not isinstance(e.after_ms, bool) and e.after_ms >= 0:
            error["afterMs"] = e.after_ms
        send({"type": "done", "invoke": m["invoke"], "error": error})
    except Exception as e:  # noqa: BLE001 확장 코드의 모든 실패를 서버에 보고한다
        send({"type": "done", "invoke": m["invoke"], "error": {"code": "WIRE.EXTENSION_ERROR", "message": str(e)}})


async def main():
    loop = asyncio.get_running_loop()
    # StreamReader's limit counts bytes before `\n`; the host frame cap includes it.
    reader = asyncio.StreamReader(limit=MAX_FRAME_BYTES - 1)
    await loop.connect_read_pipe(lambda: asyncio.StreamReaderProtocol(reader), sys.stdin)
    while line := await reader.readline():
        m = json.loads(line)
        if m["type"] == "reply":
            fut = pending.pop(m["call"])
            # 확장이 기다리던 ctx 작업을 취소했으면 응답을 버린다(r6 F04). 서버 쪽 작업은 이미 끝났다.
            if fut.done():
                continue
            if m.get("error"):
                fut.set_exception(AipError(m["error"]["code"]))
            else:
                fut.set_result(m["value"])
        elif m["type"] == "invoke":
            asyncio.create_task(run_invoke(m))


asyncio.run(main())

# AIP 확장 worker(Python). 서버와 stdin/stdout JSON 줄로만 통신한다. DB 연결 정보는 받지 않는다.
import asyncio, importlib.util, inspect, json, os, sys

ext_dir = sys.argv[1]
pending = {}
seq = 0


def send(m):
    sys.stdout.write(json.dumps(m, ensure_ascii=False) + "\n")
    sys.stdout.flush()


class AipError(Exception):
    def __init__(self, code):
        super().__init__(code)
        self.code = code


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

    # durable job 전용: 공개 read 요청을 job 주체의 정책으로 실행한다.
    async def read(self, query):
        return await self.ctx.call("read", query.get("read") if isinstance(query, dict) else None, query)


class Http:
    def __init__(self, ctx): self.ctx = ctx
    async def call(self, name, inp, key=None): return await self.ctx.call("http", name, {"input": inp, "key": key})

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

class Ctx:
    def __init__(self, token, job=None):
        self.token = token
        # durable job 전용: 실행 id와 몇 번째 시도인지(1부터). job이 아니면 None이다.
        self.job = job
        self.data = Data(self)
        self.http = Http(self)
        self.files = Files(self)
        self.events = Events(self)
        self.jobs = Jobs(self)
        self.messages = Messages(self)

    # durable job 전용: `raise ctx.retry_later("이유")`로 같은 job을 backoff 뒤 다시 시도하게 한다.
    def retry_later(self, reason=None):
        return AipError("RETRY_LATER")

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
        send({"type": "done", "invoke": m["invoke"], "error": {"code": e.code, "message": e.code}})
    except Exception as e:  # noqa: BLE001 확장 코드의 모든 실패를 서버에 보고한다
        send({"type": "done", "invoke": m["invoke"], "error": {"code": "EXTENSION_ERROR", "message": str(e)}})


async def main():
    loop = asyncio.get_running_loop()
    reader = asyncio.StreamReader()
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

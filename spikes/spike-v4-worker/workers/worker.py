# AIP 확장 worker(Python). 서버와 stdin/stdout JSON 줄로만 통신한다. DB 연결 정보는 받지 않는다.
import asyncio, importlib.util, json, os, sys

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

    async def apply(self, res, name, ids):
        return await self.ctx.call("apply", f"{res}.{name}", {"ids": [str(i) for i in ids]})


class Ctx:
    def __init__(self, token):
        self.token = token
        self.data = Data(self)

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
        out = await getattr(load(mod), fn)(m["input"], Ctx(m["token"]))
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

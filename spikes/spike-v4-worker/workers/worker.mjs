// AIP 확장 worker(Node). 서버와 stdin/stdout JSON 줄로만 통신한다. DB 연결 정보는 받지 않는다.
import { createInterface } from "node:readline";
import { pathToFileURL } from "node:url";
import path from "node:path";

const extDir = process.argv[1];
const pending = new Map();
let seq = 0;
const send = (m) => process.stdout.write(JSON.stringify(m) + "\n");

function makeCtx(token) {
  const call = (op, name, input) =>
    new Promise((resolve, reject) => {
      const id = ++seq;
      pending.set(id, { resolve, reject });
      send({ type: "call", call: id, token, op, name, input });
    });
  return {
    data: {
      aggregate: (res, name, input) => call("aggregate", `${res}.${name}`, input),
      // 쓰기 확장: 공개 전이를 서버 트랜잭션 안에서 부른다. 커밋은 서버가 한다.
      apply: (res, name, ids) => call("apply", `${res}.${name}`, { ids: ids.map(String) }),
    },
    // 시험용: 서버가 허용하지 않은 원시 요청을 보내 본다.
    _raw: (m) => new Promise((resolve, reject) => { const id = ++seq; pending.set(id, { resolve, reject }); send({ ...m, type: "call", call: id, token }); }),
  };
}

const rl = createInterface({ input: process.stdin });
rl.on("line", async (line) => {
  const m = JSON.parse(line);
  if (m.type === "reply") {
    const p = pending.get(m.call);
    pending.delete(m.call);
    if (m.error) p.reject(Object.assign(new Error(m.error.code), { code: m.error.code }));
    else p.resolve(m.value);
    return;
  }
  if (m.type === "invoke") {
    const [mod, fn] = m.impl.split(".");
    try {
      const ext = await import(pathToFileURL(path.join(extDir, `${mod}.mjs`)).href);
      const out = await ext[fn](m.input, makeCtx(m.token));
      send({ type: "done", invoke: m.invoke, output: out });
    } catch (e) {
      send({ type: "done", invoke: m.invoke, error: { code: e.code ?? "EXTENSION_ERROR", message: String(e.message ?? e) } });
    }
  }
});

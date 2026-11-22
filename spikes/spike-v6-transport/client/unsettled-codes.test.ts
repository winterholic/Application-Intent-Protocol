// D3: 응답 유실 뒤의 재시도가 "실행 전 거절"을 받으면 그 거절은 앞선 시도의 롤백 증거가 아니다. 서버가 멱등 기록을 보기 전에 돌려주는
// 코드(요청 한도, 배포 불일치, 동시 실행 상한 등)는 모두 미확정으로 남아야 한다. 그렇지 않으면 이미 커밋된 쓰기를 실패로 보고
// 호출자가 새 키로 다시 보내 두 번 실행된다.
import { test } from "node:test";
import assert from "node:assert/strict";
import { connect as rawConnect, WriteUnsettled } from "./transport.ts";

const request = { apply: "MemberAlarm.read", target: { ids: ["1"] } };
const reply = (body: unknown) => new Response(JSON.stringify(body));

function connect(impl: typeof fetch) {
  return rawConnect("http://spike.invalid", "session", async (url, options) => {
    if (String(url).endsWith("/session")) return reply({ ok: true, principal: { actorId: "1" }, remainingMs: 60_000 });
    return impl(url, options);
  });
}

// 서버가 /apply에서 멱등 조회보다 먼저 돌려줄 수 있는 코드: 요청 한도(admission), 배포 fence, 동시 실행 상한.
const BEFORE_THE_JOURNAL = ["WIRE.RATE_LIMITED", "WIRE.DEPLOYMENT_CHANGED", "WIRE.SCHEMA_MISMATCH", "WIRE.SERVER_BUSY", "WIRE.CIRCUIT_OPEN"];

for (const code of BEFORE_THE_JOURNAL) {
  test(`응답 유실 뒤 재시도가 ${code}를 받아도 키는 미확정으로 남고, 복구되면 저장된 결과를 받음`, async () => {
    let step = 0;
    const seenKeys: string[] = [];
    const fetcher: typeof fetch = async (_url, options) => {
      const body = JSON.parse(String(options?.body));
      seenKeys.push(body.key);
      step++;
      if (step === 1) throw new TypeError("response lost after commit");
      if (step <= 3) return reply({ ok: false, code, retry: "later" });
      return reply({ ok: true, changed: ["1"], unchanged: [], tags: ["MemberAlarm"], replayed: true });
    };
    const client = connect(fetcher);
    await assert.rejects(client.apply(request, { key: "one", retries: 2 }), (error: unknown) => error instanceof WriteUnsettled && (error as WriteUnsettled).retry === "later");
    assert.deepEqual(client.pending(), ["one"], "the possibly committed write keeps its key");
    const settled = await client.retryPending();
    assert.equal(settled.length, 1);
    assert.equal((settled[0] as { replayed?: boolean }).replayed, true, "the stored result is replayed, not executed again");
    assert.deepEqual(client.pending(), []);
    assert.ok(seenKeys.every((key) => key === "one"), "every attempt used the same key");
  });
}

test("첫 시도가 곧바로 요청 한도에 걸리면 실행된 적이 없으므로 최종 실패로 돌려주고 키를 남기지 않음", async () => {
  const fetcher: typeof fetch = async () => reply({ ok: false, code: "WIRE.RATE_LIMITED", retry: "later" });
  const client = connect(fetcher);
  const result = await client.apply(request, { key: "one" });
  assert.equal(result.ok, false);
  assert.equal(result.ok === false && result.code, "WIRE.RATE_LIMITED");
  assert.deepEqual(client.pending(), []);
});

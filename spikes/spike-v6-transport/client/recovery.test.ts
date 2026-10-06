import { test } from "node:test";
import assert from "node:assert/strict";
import { connect as rawConnect, WriteUnsettled } from "./transport.ts";

const request = { apply: "MemberAlarm.read", target: { ids: ["1"] } };
const query = { read: "MemberAlarm", select: ["id"] };
const reply = (body: unknown) => new Response(JSON.stringify(body));

// 전송 실패 실험과 서버 principal 확인을 분리한다. 세션 계약 반례는 session.test.ts에서 검사한다.
function connect(base: string, token: string | null, impl: typeof fetch) {
  return rawConnect(base, token, async (url, options) => {
    if (String(url).endsWith("/session")) return reply({ ok: true, principal: { actorId: "1" }, remainingMs: 60_000 });
    return impl(url, options);
  });
}

test("같은 미확정 키를 여러 번 재시도해도 확정 뒤 캐시 저장을 재개", async () => {
  let offline = true;
  const fetcher: typeof fetch = async (_url, options) => {
    if (offline) throw new TypeError("offline");
    const body = JSON.parse(String(options?.body));
    return reply(body.query ? { ok: true, rows: [1], deps: ["MemberAlarm"], maxAgeMs: 1000 } : { ok: true, tags: ["MemberAlarm"] });
  };
  const client = connect("http://spike.invalid", "session", fetcher);
  await assert.rejects(client.apply(request, { key: "one", retries: 0 }), WriteUnsettled);
  await assert.rejects(client.apply(request, { key: "one", retries: 0 }), WriteUnsettled);
  assert.deepEqual(client.pending(), ["one"]);
  offline = false;
  await client.retryPending();
  assert.deepEqual(client.pending(), []);
  assert.equal((await client.read(query)).stored, true);
});

test("응답 유실 뒤 인증 만료는 이전 쓰기의 롤백 증거가 아님", async () => {
  let mode = "lost";
  const fetcher: typeof fetch = async () => {
    if (mode === "lost") throw new TypeError("response lost after commit");
    if (mode === "expired") return reply({ ok: false, code: "TOKEN_EXPIRED" });
    return reply({ ok: true, tags: ["MemberAlarm"], replayed: true });
  };
  const client = connect("http://spike.invalid", "session", fetcher);
  await assert.rejects(client.apply(request, { key: "one", retries: 0 }), WriteUnsettled);
  mode = "expired";
  assert.deepEqual(await client.retryPending(), []);
  assert.deepEqual(client.pending(), ["one"]);
  mode = "restored";
  assert.equal((await client.retryPending())[0].replayed, true);
  assert.deepEqual(client.pending(), []);
});

test("미확정 키를 다른 본문으로 재사용해도 원래 복구 요청을 잃지 않음", async () => {
  let offline = true;
  const seen: unknown[] = [];
  const fetcher: typeof fetch = async (_url, options) => {
    const body = JSON.parse(String(options?.body));
    seen.push(body.request);
    if (offline) throw new TypeError("offline");
    return reply({ ok: true, tags: ["MemberAlarm"], replayed: true });
  };
  const client = connect("http://spike.invalid", "session", fetcher);
  await assert.rejects(client.apply(request, { key: "one", retries: 0 }), WriteUnsettled);
  const other = { apply: "MemberAlarm.read", target: { ids: ["2"] } };
  const mismatch = await client.apply(other, { key: "one", retries: 0 }).catch((e) => ({ code: e.code }));
  assert.equal(mismatch.code, "IDEMPOTENCY_MISMATCH");
  offline = false;
  await client.retryPending();
  assert.deepEqual(seen.at(-1), request);
});

test("같은 키의 동시 호출은 하나의 전송과 복구 결과를 공유", async () => {
  let release!: () => void;
  const gate = new Promise<void>((resolve) => (release = resolve));
  let calls = 0;
  const fetcher: typeof fetch = async () => {
    calls++;
    if (calls === 1) {
      await gate;
      throw new TypeError("response lost");
    }
    return reply({ ok: true, tags: ["MemberAlarm"], replayed: true });
  };
  const client = connect("http://spike.invalid", "session", fetcher);
  const a = client.apply(request, { key: "same" });
  const b = client.apply(request, { key: "same" });
  await new Promise<void>((resolve) => setImmediate(resolve));
  assert.equal(calls, 1);
  const mismatch = await client.apply({ ...request, target: { ids: ["2"] } }, { key: "same" });
  assert.equal(mismatch.code, "IDEMPOTENCY_MISMATCH");
  release();
  assert.deepEqual(await a, await b);
  assert.equal(calls, 2);
  assert.deepEqual(client.pending(), []);
});

test("원본과 오류의 request를 바꿔도 복구는 최초 JSON을 사용", async () => {
  const original = structuredClone(request);
  let offline = true;
  let sent: unknown;
  const fetcher: typeof fetch = async (_url, options) => {
    sent = JSON.parse(String(options?.body)).request;
    if (offline) throw new TypeError("offline");
    return reply({ ok: true, tags: ["MemberAlarm"] });
  };
  const client = connect("http://spike.invalid", "session", fetcher);
  const error = await client.apply(original, { key: "one", retries: 0 }).catch((e) => e);
  original.target.ids[0] = "2";
  error.request.target.ids[0] = "3";
  offline = false;
  await client.retryPending();
  assert.deepEqual(sent, request);
});

test("JSON으로 보낼 수 없는 입력은 전송 전 오류로 처리", async () => {
  let calls = 0;
  const client = connect("http://spike.invalid", "session", async () => {
    calls++;
    throw new TypeError("unexpected network");
  });
  await assert.rejects(client.apply({ id: 1n }, { key: "bigint", retries: 0 }), TypeError);
  assert.equal(calls, 0);
  assert.deepEqual(client.pending(), []);
});

test("잘못된 성공 응답은 커밋 확인으로 받아들이지 않음", async () => {
  for (const response of [{}, { ok: true }, { ok: false }, { ok: true, tags: [1] }]) {
    const client = connect("http://spike.invalid", "session", async () => reply(response));
    await assert.rejects(client.apply(request, { key: "one", retries: 0 }), WriteUnsettled);
    assert.deepEqual(client.pending(), ["one"]);
  }
});

test("최초 전송의 실행 전 거부는 미확정 쓰기를 남기지 않음", async () => {
  for (const code of ["TOKEN_EXPIRED", "UNAUTHENTICATED", "BAD_REQUEST", "PAYLOAD_TOO_LARGE", "NOT_FOUND"]) {
    const client = connect("http://spike.invalid", "session", async (url) => reply(String(url).endsWith("/read")
      ? { ok: true, rows: [1], deps: ["MemberAlarm"], maxAgeMs: 1000 }
      : { ok: false, code }));
    assert.deepEqual(await client.apply(request, { key: code, retries: 0 }), { ok: false, code });
    assert.deepEqual(client.pending(), []);
    assert.equal((await client.read(query)).stored, true);
  }
});

test("앞선 응답이 유실됐다면 실행 전 거부만으로 pending을 제거하지 않음", async () => {
  for (const code of ["TOKEN_EXPIRED", "UNAUTHENTICATED", "BAD_REQUEST", "PAYLOAD_TOO_LARGE", "NOT_FOUND"]) {
    let calls = 0;
    const client = connect("http://spike.invalid", "session", async () => {
      if (++calls === 1) throw new TypeError("response lost");
      return reply({ ok: false, code });
    });
    await assert.rejects(client.apply(request, { key: code, retries: 1 }), WriteUnsettled);
    assert.deepEqual(client.pending(), [code]);
  }
});

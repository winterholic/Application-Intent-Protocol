import { test } from "node:test";
import assert from "node:assert/strict";
import { connect, WriteUnsettled } from "./transport.ts";

const fingerprint = "a".repeat(64);
const session = { ok: true, principal: { actorId: "1" }, remainingMs: 60000 };
const response = (body: unknown) => new Response(JSON.stringify(body));
const request = { apply: "Inbox.mark", target: { ids: ["42"] } };
const success = { ok: true, changed: ["42"], unchanged: [], tags: ["Inbox"], replayed: true };

function clientFor(write: (attempt: number) => unknown, idWire: "safe-number-v13" | "decimal-string-v13" = "decimal-string-v13") {
  let attempts = 0;
  const fetcher: typeof fetch = async (url) => {
    const path = new URL(String(url)).pathname;
    if (path === "/session") return response(session);
    if (path === "/read") return response({ ok: true, rows: [{ id: "42" }], deps: ["Inbox"], maxAgeMs: 5000, contractFingerprint: fingerprint });
    return response(write(++attempts));
  };
  return connect("http://envelope.invalid", "token", fetcher, { contractFingerprint: fingerprint, idWire });
}

test("V14: 누락되거나 형식이 틀린 성공 배열은 pending으로 남고 같은 키 재생으로 복구", async () => {
  for (const malformed of [
    { ok: true, tags: [] },
    { ...success, changed: null },
    { ...success, unchanged: {} },
    { ...success, changed: [42] },
    { ...success, unchanged: [null] },
    { ...success, tags: [1] },
    { ...success, replayed: "yes" },
    { ...success, recovered: 1 },
  ]) {
    const aip = clientFor(n => n === 1 ? malformed : success);
    await assert.rejects(aip.apply(request, { key: "write", retries: 0 }), WriteUnsettled);
    assert.deepEqual(aip.pending(), ["write"]);
    assert.equal((await aip.read({ read: "Inbox", select: ["id"] })).stored, false);
    const outcomes = await aip.retryPending();
    assert.deepEqual(outcomes[0].changed, ["42"]);
    assert.equal(outcomes[0].replayed, true);
    assert.deepEqual(aip.pending(), []);
    assert.equal((await aip.read({ read: "Inbox", select: ["id"] })).stored, true);
  }
});

test("V14: 문자열 Id는 canonical 비음수 i64이며 changed와 unchanged를 모두 검사", async () => {
  for (const value of ["", "00", "01", "-1", "+1", " 1", "1.0", "9223372036854775808", "9".repeat(100), false]) {
    for (const field of ["changed", "unchanged"]) {
      const aip = clientFor(() => ({ ...success, [field]: [value] }));
      await assert.rejects(aip.apply(request, { key: "bad", retries: 0 }), WriteUnsettled);
      assert.deepEqual(aip.pending(), ["bad"]);
    }
  }
  const aip = clientFor(() => ({ ...success, changed: ["0", "9223372036854775807"] }));
  assert.deepEqual((await aip.apply(request)).changed, ["0", "9223372036854775807"]);
});

test("V14: 숫자 Id는 안전 정수이며 다른 mode와 범위 밖 응답을 확정하지 않음", async () => {
  for (const value of ["42", -1, 0.5, 9007199254740992, null, {}]) {
    const aip = clientFor(() => ({ ok: true, tags: [], changed: [], unchanged: [value] }), "safe-number-v13");
    await assert.rejects(aip.apply(request, { key: "bad", retries: 0 }), WriteUnsettled);
  }
  const aip = clientFor(() => ({ ok: true, tags: [], changed: [0, 9007199254740991], unchanged: [42] }), "safe-number-v13");
  assert.deepEqual((await aip.apply(request)).changed, [0, 9007199254740991]);
});

test("V14: 같은 actor 세션 교체 뒤에도 malformed 성공은 미확정으로 유지", async () => {
  const aip = clientFor(n => n < 3 ? { ...success, unchanged: [42] } : success);
  await assert.rejects(aip.apply(request, { key: "renew", retries: 0 }), WriteUnsettled);
  assert.deepEqual(await aip.replaceSession("new-token"), []);
  assert.deepEqual(aip.pending(), ["renew"]);
  assert.equal((await aip.retryPending())[0].ok, true);
  assert.deepEqual(aip.pending(), []);
});

test("V14: 복구된 확정 오류도 code와 recovered를 보존", async () => {
  const aip = clientFor(n => n === 1 ? { ...success, changed: null } : { ok: false, code: "FORBIDDEN", msg: "denied" });
  const outcome = await aip.apply(request, { key: "rejected", retries: 1 });
  assert.equal(outcome.ok, false);
  assert.equal(outcome.code, "FORBIDDEN");
  assert.equal(outcome.recovered, true);
  assert.deepEqual(aip.pending(), []);
});

test("V14: 복구 목록의 key는 서버의 추가 속성으로 덮어쓰지 않음", async () => {
  const aip = clientFor(n => n === 1 ? { ...success, changed: null } : { ...success, key: 123 });
  await assert.rejects(aip.apply(request, { key: "local-key", retries: 0 }), WriteUnsettled);
  assert.equal((await aip.retryPending())[0].key, "local-key");
});

test("V14: 같은 키 응답과 복구 목록은 JS 호출자의 변조도 차단", async () => {
  const aip = clientFor(() => success);
  const [first, second] = await Promise.all([aip.apply(request, { key: "shared" }), aip.apply(request, { key: "shared" })]);
  assert.equal(Object.isFrozen(first), true);
  for (const field of ["changed", "unchanged", "tags"]) assert.equal(Object.isFrozen(first[field]), true);
  assert.throws(() => first.changed.push("other"), TypeError);
  assert.deepEqual(second.changed, ["42"]);
  const recovery = clientFor(n => n === 1 ? { ...success, changed: null } : success);
  await assert.rejects(recovery.apply(request, { key: "recover", retries: 0 }), WriteUnsettled);
  const outcomes = await recovery.retryPending();
  assert.equal(Object.isFrozen(outcomes), true);
  assert.equal(Object.isFrozen(outcomes[0]), true);
  assert.equal(Object.isFrozen(outcomes[0].changed), true);
  const rejected = clientFor(n => n === 1 ? { ...success, changed: null } : { ok: false, code: "FORBIDDEN" });
  const outcome = await rejected.apply(request, { key: "error", retries: 1 });
  assert.equal(outcome.recovered, true);
  assert.equal(Object.isFrozen(outcome), true);
});

test("V14: 같은 키의 다른 본문 사전 거부도 불변 결과이며 원래 pending 보존", async () => {
  const aip = clientFor(() => ({ ...success, changed: null }));
  await assert.rejects(aip.apply(request, { key: "fixed", retries: 0 }), WriteUnsettled);
  const rejected = await aip.apply({ apply: "Inbox.mark", target: { ids: ["43"] } }, { key: "fixed" });
  assert.equal(rejected.code, "IDEMPOTENCY_MISMATCH");
  assert.equal(Object.isFrozen(rejected), true);
  assert.deepEqual(aip.pending(), ["fixed"]);
});

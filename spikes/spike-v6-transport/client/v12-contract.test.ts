import { test } from "node:test";
import assert from "node:assert/strict";
import { connectTyped } from "./typed.ts";
import { contract, contractFingerprint } from "./generated-v12.ts";

const query = { read: "MemberAlarm", select: ["id", "isChecked"] } as const;
const reply = (body: unknown) => new Response(JSON.stringify(body), { headers: { "content-type": "application/json" } });
const session = { ok: true, principal: { actorId: "1" }, remainingMs: 60_000 };
const readBody = (overrides: Record<string, unknown> = {}) => ({
  ok: true,
  rows: [{ id: 1, isChecked: false }],
  deps: ["MemberAlarm"],
  maxAgeMs: 1000,
  contractFingerprint,
  ...overrides,
});

function fetchWith(read: (query: unknown, call: number) => unknown, writes?: () => unknown): typeof fetch {
  let reads = 0;
  return async (input, init) => {
    const path = new URL(String(input)).pathname;
    if (path === "/session") return reply(session);
    const body = JSON.parse(String(init?.body));
    if (path === "/read") return reply(read(body.query, ++reads));
    if (path === "/apply") return reply(writes?.() ?? { ok: true, tags: ["MemberAlarm"] });
    return reply({ ok: false, code: "NOT_FOUND" });
  };
}

function isCode(code: string) {
  return (error: any) => error?.code === code;
}

test("typed facade는 검사하지 않은 행을 넣을 캐시 메서드를 공개하지 않음", async () => {
  const client = connectTyped("http://contract.invalid", "token", contract, fetchWith(() => readBody()));
  assert.deepEqual(Object.keys(client.cache), ["size"]);
  assert.equal((client.cache as any).read, undefined);
  assert.equal((await client.read(query)).rows[0].isChecked, false);
  assert.equal(client.cache.size(), 1);
});

test("typed binding 식별값이 빠지거나 잘못되면 요청 전에 거부하고 생성 시 값을 고정", async () => {
  let calls = 0;
  const fetcher: typeof fetch = async () => { calls++; return reply(session); };
  for (const fingerprint of [undefined, null, 7, "", "0".repeat(63), "G".repeat(64)]) {
    assert.throws(() => connectTyped("http://contract.invalid", "token", { ...contract, fingerprint } as any, fetcher), isCode("PROTOCOL_ERROR"));
  }
  assert.equal(calls, 0);
  const mutable = { ...contract };
  const client = connectTyped("http://contract.invalid", "token", mutable, fetchWith(() => readBody()));
  mutable.fingerprint = "f".repeat(64);
  assert.equal((await client.read(query)).stored, true);
});

test("계약 불일치로 비운 뒤에도 미확정 쓰기가 있으면 캐시 저장을 보류", async () => {
  let mismatch = true;
  let writeAvailable = false;
  const fetcher: typeof fetch = async (input) => {
    const path = new URL(String(input)).pathname;
    if (path === "/session") return reply(session);
    if (path === "/apply") {
      if (!writeAvailable) throw new Error("응답 유실");
      return reply({ ok: true, tags: ["MemberAlarm"] });
    }
    return reply(readBody({ contractFingerprint: mismatch ? "0".repeat(64) : contractFingerprint }));
  };
  const client = connectTyped("http://contract.invalid", "token", contract, fetcher);
  await assert.rejects(client.apply({ apply: "MemberAlarm.read", target: { ids: ["1"] } }, { key: "pending", retries: 0 }));
  await assert.rejects(client.read(query), isCode("CONTRACT_MISMATCH"));
  mismatch = false;
  const fresh = await client.read(query);
  assert.equal(fresh.stored, false);
  assert.equal((await client.read(query)).cached, false);
  assert.deepEqual(client.pending(), ["pending"]);
  writeAvailable = true;
  await client.retryPending();
  assert.deepEqual(client.pending(), []);
  assert.equal((await client.read(query)).stored, true);
  assert.equal((await client.read(query)).cached, true);
});

test("누락·null·비문자열·다른 fingerprint는 행 검사보다 먼저 실패하고 저장하지 않음", async () => {
  assert.equal(contract.fingerprint, contractFingerprint, "앱 binding과 생성 모듈의 fingerprint export가 같아야 함");
  const badValues: [string, Record<string, unknown>][] = [
    ["missing", { contractFingerprint: undefined }],
    ["null", { contractFingerprint: null }],
    ["non-string", { contractFingerprint: 7 }],
    ["mismatch", { contractFingerprint: "0".repeat(64) }],
  ];
  for (const [name, override] of badValues) {
    let calls = 0;
    const client = connectTyped("http://contract.invalid", "token", contract, fetchWith((_query, call) => {
      calls++;
      // 첫 응답은 행 모양도 틀렸다. 계약 확인이 앞서므로 오류는 fingerprint mismatch여야 한다.
      return call === 1
        ? readBody({ rows: "malformed", ...override })
        : readBody();
    }));
    await assert.rejects(client.read(query), isCode("CONTRACT_MISMATCH"), name);
    const next = await client.read(query);
    assert.equal(next.cached, false, `${name}: 실패 응답은 캐시되지 않음`);
    assert.equal(next.rows[0].isChecked, false);
    assert.equal(calls, 2);
  }
});

test("올바른 계약에서도 성공 envelope 구조 오류와 서버 오류 코드를 구분", async () => {
  const malformed: [string, Record<string, unknown>][] = [
    ["rows", { rows: {} }],
    ["deps", { deps: ["MemberAlarm", 1] }],
    ["maxAgeMs", { maxAgeMs: "1000" }],
    ["ok", { ok: "true" }],
  ];
  for (const [name, override] of malformed) {
    const client = connectTyped("http://contract.invalid", "token", contract,
      fetchWith(() => readBody(override)));
    await assert.rejects(client.read(query), isCode("PROTOCOL_ERROR"), name);
    assert.equal(client.cache.size(), 0, `${name}: 잘못된 envelope는 저장되지 않음`);
  }

  const rejected = connectTyped("http://contract.invalid", "token", contract,
    fetchWith(() => ({ ok: false, code: "FIELD_NOT_EXPOSED" })));
  await assert.rejects(rejected.read(query), isCode("FIELD_NOT_EXPOSED"));
  assert.equal(rejected.cache.size(), 0);
});

test("일치하는 fingerprint 결과는 캐시되고 TTL 뒤에는 다시 읽음", async () => {
  let calls = 0;
  const client = connectTyped("http://contract.invalid", "token", contract,
    fetchWith((_query, call) => {
      calls++;
      return readBody({ rows: [{ id: call, isChecked: false }], maxAgeMs: 25 });
    }));

  const first = await client.read(query);
  assert.equal(first.cached, false);
  assert.equal(first.stored, true);
  const hit = await client.read(query);
  assert.equal(hit.cached, true);
  assert.equal(hit.rows[0].id, 1);
  assert.equal(calls, 1);

  await new Promise((resolve) => setTimeout(resolve, 40));
  const expired = await client.read(query);
  assert.equal(expired.cached, false);
  assert.equal(expired.rows[0].id, 2);
  assert.equal(calls, 2);
});

test("읽기 중 쓰기가 캐시 epoch을 바꾸면 이전 행을 버리고 새 결과를 읽음", async () => {
  let releaseFirst!: (response: Response) => void;
  let markStarted!: () => void;
  const firstStarted = new Promise<void>((resolve) => { markStarted = resolve; });
  let reads = 0;
  const fetcher: typeof fetch = async (input, init) => {
    const path = new URL(String(input)).pathname;
    if (path === "/session") return reply(session);
    if (path === "/apply") return reply({ ok: true, tags: ["MemberAlarm"] });
    if (path === "/read") {
      reads++;
      if (reads === 1) return new Promise<Response>((resolve) => {
        releaseFirst = resolve;
        markStarted();
      });
      return reply(readBody({ rows: [{ id: 2, isChecked: true }] }));
    }
    return reply({ ok: false, code: "NOT_FOUND" });
  };
  const client = connectTyped("http://contract.invalid", "token", contract, fetcher);
  const pending = client.read(query);
  await firstStarted;
  await client.apply({ apply: "MemberAlarm.read", target: { ids: ["1"] } });
  releaseFirst(reply(readBody({ rows: [{ id: 1, isChecked: false }] })));
  const fresh = await pending;
  assert.equal(reads, 2);
  assert.equal(fresh.rows[0].id, 2);
  assert.equal(fresh.rows[0].isChecked, true);
  assert.equal(fresh.stale, false);
});

test("다른 query의 fingerprint mismatch는 기존 캐시도 비우고 matching 응답으로 복구", async () => {
  let mismatch = false;
  let calls = 0;
  const client = connectTyped("http://contract.invalid", "token", contract,
    fetchWith((q) => {
      calls++;
      const isOtherQuery = (q as any).read === "Recruitment";
      return readBody({
        rows: [{ id: calls, isChecked: false }],
        contractFingerprint: mismatch && isOtherQuery ? "f".repeat(64) : contractFingerprint,
      });
    }));
  await client.read(query);
  assert.equal((await client.read(query)).cached, true);

  mismatch = true;
  await assert.rejects(client.read({ read: "Recruitment", select: ["id"] }), isCode("CONTRACT_MISMATCH"));
  mismatch = false;
  const refetched = await client.read(query);
  assert.equal(refetched.cached, false, "fingerprint mismatch 뒤 기존 query 캐시는 hit되면 안 됨");
  assert.equal(calls, 3);
});

test("세션 교체 뒤 늦게 온 구세대 fingerprint 오류가 새 캐시를 지우지 않고 ScopeChanged 우선", async () => {
  let releaseOld!: (response: Response) => void;
  let markOldStarted!: () => void;
  const oldStarted = new Promise<void>((resolve) => { markOldStarted = resolve; });
  let reads = 0;
  const fetcher: typeof fetch = async (input, init) => {
    const path = new URL(String(input)).pathname;
    const token = new Headers(init?.headers).get("authorization")?.slice(7);
    if (path === "/session") return reply({ ok: true, principal: { actorId: "1" }, remainingMs: 60_000 });
    if (path === "/read") {
      reads++;
      if (token === "old") return new Promise<Response>((resolve) => {
        releaseOld = resolve;
        markOldStarted();
      });
      return reply(readBody({ rows: [{ id: 2, isChecked: true }] }));
    }
    return reply({ ok: true, tags: [] });
  };
  const client = connectTyped("http://contract.invalid", "old", contract, fetcher);
  const oldRead = client.read(query);
  const rejected = assert.rejects(oldRead, (error: any) => error?.name === "ScopeChanged");
  await oldStarted;
  await client.replaceSession("new");
  const current = await client.read(query);
  assert.equal(current.rows[0].id, 2);

  releaseOld(reply(readBody({ contractFingerprint: "e".repeat(64) })));
  await rejected;
  const hit = await client.read(query);
  assert.equal(hit.cached, true, "구세대 응답 처리가 새 세대 캐시를 제거하면 안 됨");
  assert.equal(reads, 2);
});

test("fingerprint가 일치해도 세션 교체 전 시작한 늦은 응답은 새 연결에 반환하지 않음", async () => {
  let releaseOld!: (response: Response) => void;
  let markOldStarted!: () => void;
  const oldStarted = new Promise<void>((resolve) => { markOldStarted = resolve; });
  let reads = 0;
  const fetcher: typeof fetch = async (input, init) => {
    const path = new URL(String(input)).pathname;
    const token = new Headers(init?.headers).get("authorization")?.slice(7);
    if (path === "/session") return reply({ ok: true, principal: { actorId: "1" }, remainingMs: 60_000 });
    if (path === "/read") {
      reads++;
      if (token === "old") return new Promise<Response>((resolve) => {
        releaseOld = resolve;
        markOldStarted();
      });
      return reply(readBody({ rows: [{ id: 3, isChecked: true }] }));
    }
    return reply({ ok: true, tags: [] });
  };
  const client = connectTyped("http://contract.invalid", "old", contract, fetcher);
  const oldRead = client.read(query);
  const rejected = assert.rejects(oldRead, (error: any) => error?.name === "ScopeChanged");
  await oldStarted;
  await client.replaceSession("new");
  await client.read(query);

  releaseOld(reply(readBody({ rows: [{ id: 1, isChecked: false }] })));
  await rejected;
  assert.equal((await client.read(query)).cached, true);
  assert.equal(reads, 2);
});

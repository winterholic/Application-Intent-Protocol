import { test } from "node:test";
import assert from "node:assert/strict";
import { connect, WriteUnsettled } from "./transport.ts";
import { ScopeChanged } from "../../spike-v5-sdk/sdk/cache.ts";

const request = { apply: "MemberAlarm.read", target: { ids: ["1"] } };
const query = { read: "MemberAlarm", select: ["id"] };
const reply = (body: unknown) => new Response(JSON.stringify(body));
const tick = () => new Promise<void>((resolve) => setImmediate(resolve));

function backend(effect: (path: string, token: string, body: any) => Promise<Response>): typeof fetch {
  return async (url, opts) => {
    const path = new URL(String(url)).pathname;
    const token = new Headers(opts?.headers).get("authorization")?.slice(7) ?? "";
    if (path === "/session") {
      if (token === "invalid") return reply({ ok: false, code: "UNAUTHENTICATED" });
      return reply({ ok: true, principal: { actorId: token === "other" ? "2" : token ? "1" : null }, remainingMs: token ? 60_000 : null });
    }
    return effect(path, token, JSON.parse(String(opts?.body)));
  };
}

test("같은 사용자 토큰 갱신은 원래 키·본문의 미확정 쓰기를 자동 복구", async () => {
  let seen: any;
  const client = connect("http://spike.invalid", "old", backend(async (path, token, body) => {
    if (path === "/read") return reply({ ok: true, rows: [1], deps: ["MemberAlarm"], maxAgeMs: 1000 });
    if (token === "old") throw new TypeError("lost after commit");
    seen = body;
    return reply({ ok: true, tags: ["MemberAlarm"], replayed: true });
  }));
  await client.read(query);
  await assert.rejects(client.apply(request, { key: "one", retries: 0 }), WriteUnsettled);
  const recovered = await client.replaceSession("new");
  assert.equal(recovered[0].replayed, true);
  assert.deepEqual(seen, { request, key: "one" });
  assert.deepEqual(client.pending(), []);
  assert.equal((await client.read(query)).stored, true);
});

test("다른 사용자·잘못된 토큰은 원래 연결과 복구 정보를 유지", async () => {
  const seen: string[] = [];
  const client = connect("http://spike.invalid", "old", backend(async (_path, token) => {
    seen.push(token);
    throw new TypeError("lost");
  }));
  await assert.rejects(client.apply(request, { key: "one", retries: 0 }), WriteUnsettled);
  await assert.rejects(client.replaceSession("other"), (e: any) => e.name === "ScopeConflict");
  await assert.rejects(client.replaceSession("invalid"), (e: any) => e.code === "UNAUTHENTICATED");
  await client.retryPending();
  assert.deepEqual(client.pending(), ["one"]);
  assert.ok(seen.every((token) => token === "old"));
});

test("교체 뒤 도착한 이전 세대 읽기는 캐시와 호출자에게 반환하지 않음", async () => {
  let release!: () => void;
  const gate = new Promise<void>((resolve) => (release = resolve));
  const client = connect("http://spike.invalid", "old", backend(async (_path, token) => {
    if (token === "old") await gate;
    return reply({ ok: true, rows: [token], deps: ["MemberAlarm"], maxAgeMs: 1000 });
  }));
  const old = client.read(query);
  const rejected = assert.rejects(old, ScopeChanged);
  await tick();
  await client.replaceSession("new");
  release();
  await rejected;
  assert.equal(client.cache.size(), 0);
  assert.deepEqual((await client.read(query)).rows, ["new"]);
});

test("이전 세대 쓰기가 늦게 끝나도 새 토큰 복구 결과를 지우지 않음", async () => {
  let release!: () => void;
  const gate = new Promise<void>((resolve) => (release = resolve));
  let newCalls = 0;
  const client = connect("http://spike.invalid", "old", backend(async (_path, token) => {
    if (token === "old") {
      await gate;
      return reply({ ok: false, code: "TOKEN_EXPIRED" });
    }
    newCalls++;
    return reply({ ok: true, tags: ["MemberAlarm"], replayed: true });
  }));
  const old = client.apply(request, { key: "one", retries: 0 });
  await tick();
  const recovered = await client.replaceSession("new");
  assert.equal(recovered[0].replayed, true);
  release();
  assert.equal((await old).replayed, true);
  assert.equal(newCalls, 1);
  assert.deepEqual(client.pending(), []);
});

test("동시 교체는 검증·교체 순서를 유지하고 같은 토큰도 읽기 세대를 바꿈", async () => {
  const seen: string[] = [];
  const client = connect("http://spike.invalid", "old", backend(async (_path, token) => {
    seen.push(token);
    return reply({ ok: true, rows: [token], deps: ["MemberAlarm"], maxAgeMs: 1000 });
  }));
  await client.read(query);
  await Promise.all([client.replaceSession("new"), client.replaceSession("latest")]);
  assert.equal((await client.read(query)).cached, false);
  assert.equal(seen.at(-1), "latest");
  await client.replaceSession("latest");
  assert.equal((await client.read(query)).cached, false);
});

test("익명 pending은 로그인 사용자에게 자동 이관하지 않음", async () => {
  const client = connect("http://spike.invalid", null, backend(async () => { throw new TypeError("lost"); }));
  await assert.rejects(client.apply(request, { key: "one", retries: 0 }), WriteUnsettled);
  await assert.rejects(client.replaceSession("new"), (e: any) => e.name === "ScopeConflict");
  assert.deepEqual(client.pending(), ["one"]);
});

test("첫 세션 확인 실패는 apply를 보내거나 pending을 만들지 않음", async () => {
  let applyCalls = 0;
  const client = connect("http://spike.invalid", "old", async (url) => {
    if (String(url).endsWith("/session")) throw new TypeError("session offline");
    applyCalls++;
    return reply({ ok: true, tags: [] });
  });
  await assert.rejects(client.apply(request, { key: "one", retries: 0 }), TypeError);
  assert.equal(applyCalls, 0);
  assert.deepEqual(client.pending(), []);
});

test("세션 actor는 정밀도 손실 없는 i64 문자열로 검증", async () => {
  for (const actorId of [1, "01", "-0", "9223372036854775808", "-9223372036854775809", null]) {
    const client = connect("http://spike.invalid", "old", async () => reply({ ok: true, principal: { actorId }, remainingMs: 1000 }));
    await assert.rejects(client.apply(request, { retries: 0 }), (e: any) => e.code === "PROTOCOL_ERROR");
    assert.deepEqual(client.pending(), []);
  }
  for (const actorId of ["9223372036854775807", "-9223372036854775808"]) {
    const client = connect("http://spike.invalid", "old", async (url) => reply(String(url).endsWith("/session")
      ? { ok: true, principal: { actorId }, remainingMs: 1000 }
      : { ok: true, tags: [] }));
    assert.equal((await client.apply(request, { retries: 0 })).ok, true);
    await client.replaceSession("new");
  }
});

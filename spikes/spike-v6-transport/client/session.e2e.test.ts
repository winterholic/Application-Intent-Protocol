import { test } from "node:test";
import assert from "node:assert/strict";
import { connect, ScopeConflict, WriteUnsettled } from "./transport.ts";

const URL = process.env.AIP_URL!;
const T1 = process.env.AIP_TOKEN_1!;
const T1_NEW = process.env.AIP_TOKEN_1_NEW!;
const T2 = process.env.AIP_TOKEN_2!;
const T1_SHORT = process.env.AIP_TOKEN_1_SHORT!;
const EXPIRED = process.env.AIP_TOKEN_EXPIRED!;
const request = { apply: "Apply.approve", target: { ids: ["300"] } };

test("세션 확인은 서버가 토큰에서 principal과 남은 수명을 결정", async () => {
  const client = connect(URL, T1);
  const session = await client.post("/session", {});
  assert.equal(session.ok, true);
  assert.equal(session.principal.actorId, "1");
  assert.ok(session.remainingMs > 0 && session.remainingMs <= 600_000);
  assert.deepEqual(await client.replaceSession(T1_NEW), []);
});

test("처음부터 만료된 토큰은 apply 사전 확인에서 거부되고 pending을 만들지 않음", async () => {
  let applyCalls = 0;
  const fetcher: typeof fetch = (input, init) => {
    if (String(input).endsWith("/apply")) applyCalls++;
    return fetch(input, init);
  };
  const expired = connect(URL, EXPIRED, fetcher);
  await assert.rejects(expired.apply(request, { key: "expired-before-apply" }), (error: any) => error.code === "TOKEN_EXPIRED");
  assert.deepEqual(expired.pending(), []);
  assert.equal(applyCalls, 0);
});

test("응답 유실 뒤 같은 principal 갱신으로 같은 키를 재확정하고 다른 principal은 거부", async () => {
  let applyCalls = 0;
  const applyBodies: string[] = [];
  const fetcher: typeof fetch = async (input, init) => {
    const url = String(input);
    if (!url.endsWith("/apply")) return fetch(input, init);

    applyCalls++;
    applyBodies.push(String(init?.body));
    if (applyCalls === 1) {
      const committedResponse = await fetch(input, init);
      assert.equal((await committedResponse.json()).ok, true);
      throw new TypeError("응답 유실을 재현");
    }
    return fetch(input, init);
  };

  const client = connect(URL, T1_SHORT, fetcher);
  const key = "v10-approve-300";
  await assert.rejects(client.apply(request, { key, retries: 0 }), WriteUnsettled);
  await new Promise((resolve) => setTimeout(resolve, 3100));
  assert.equal((await client.post("/session", {})).code, "TOKEN_EXPIRED");
  assert.deepEqual(await client.retryPending(), []);
  assert.deepEqual(client.pending(), [key]);

  await assert.rejects(client.replaceSession(EXPIRED), (error: any) => error.code === "TOKEN_EXPIRED");
  await assert.rejects(client.replaceSession(T2), ScopeConflict);
  assert.deepEqual(client.pending(), [key]);
  assert.equal(applyCalls, 2, "만료·다른 actor 세션 확인은 pending을 재전송하지 않음");

  const recovered = await client.replaceSession(T1_NEW);
  assert.equal(recovered.length, 1);
  assert.equal(recovered[0].ok, true);
  assert.equal(recovered[0].replayed, true);
  assert.deepEqual(recovered[0].changed, [300]);
  assert.deepEqual(client.pending(), []);

  const decoded = applyBodies.map((body) => JSON.parse(body));
  assert.equal(decoded.length, 3);
  assert.ok(decoded.every((body) => body.key === key && JSON.stringify(body.request) === JSON.stringify(request)));
});

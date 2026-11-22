// D3: 묶음 결과의 `applied`는 compose 단계마다 한 행을 더한다. 행 번호와 연산 번호가 어긋나므로 id 종류(Uuid·정수) 검사는
// 서버가 주는 `ops` 구간으로 행을 연산에 맞춰야 한다. 어긋난 채 검사하면 커밋된 묶음을 형식 오류로 보고 영영 미확정으로 남긴다.
import { test } from "node:test";
import assert from "node:assert/strict";
import { connect as rawConnect, WriteUnsettled } from "./transport.ts";

const reply = (body: unknown) => new Response(JSON.stringify(body));
const UUID = "11111111-1111-4111-8111-111111111111";

function connect(response: unknown) {
  return rawConnect("http://spike.invalid", "session", async (url) => {
    if (String(url).endsWith("/session")) return reply({ ok: true, principal: { actorId: "1" }, remainingMs: 60_000 });
    return reply(response);
  }, { idWire: "decimal-string-v13", identityKinds: { Account: "uuid", Product: "bigint" } });
}

const request = {
  atomic: [
    { compose: "Account", targets: { ids: [UUID] }, steps: [{ transition: "touch" }, { transition: "touch" }] },
    { apply: "Product.take", target: { ids: ["1"] }, input: { qty: 1 } },
  ],
};

test("compose가 단계마다 applied 행을 더해도 uuid 자원의 id를 ops 구간으로 맞춰 검사", async () => {
  const committed = {
    ok: true, tags: ["Account", "Product"],
    applied: [{ changed: [UUID], unchanged: [] }, { changed: [UUID], unchanged: [] }, { changed: ["1"], unchanged: [] }],
    created: [],
    ops: [{ kind: "compose", applied: [0, 2], created: [0, 0] }, { kind: "apply", applied: [2, 3], created: [0, 0] }],
  };
  const result = await connect(committed).apply(request, { key: "one", retries: 0 });
  assert.equal(result.ok, true);
});

test("ops 구간이 applied를 덮지 못하면 형식 오류로 보고 미확정으로 남김", async () => {
  const broken = {
    ok: true, tags: ["Account"],
    applied: [{ changed: [UUID], unchanged: [] }, { changed: [UUID], unchanged: [] }, { changed: ["1"], unchanged: [] }],
    created: [],
    ops: [{ kind: "compose", applied: [0, 1], created: [0, 0] }],
  };
  await assert.rejects(connect(broken).apply(request, { key: "one", retries: 0 }), WriteUnsettled);
});

test("uuid 자원에 정수 id가 오면 여전히 형식 오류", async () => {
  const wrong = {
    ok: true, tags: ["Account", "Product"],
    applied: [{ changed: ["7"], unchanged: [] }, { changed: [UUID], unchanged: [] }, { changed: ["1"], unchanged: [] }],
    created: [],
    ops: [{ kind: "compose", applied: [0, 2], created: [0, 0] }, { kind: "apply", applied: [2, 3], created: [0, 0] }],
  };
  await assert.rejects(connect(wrong).apply(request, { key: "one", retries: 0 }), WriteUnsettled);
});

test("compose 없는 묶음은 ops 없이 apply 연산 순서대로 검사", async () => {
  const plain = {
    ok: true, tags: ["Account", "Product"],
    applied: [{ changed: [UUID], unchanged: [] }, { changed: ["1"], unchanged: [] }],
    created: [],
  };
  const two = { atomic: [{ apply: "Account.touch", target: { ids: [UUID] } }, { apply: "Product.take", target: { ids: ["1"] }, input: { qty: 1 } }] };
  assert.equal((await connect(plain).apply(two, { key: "one", retries: 0 })).ok, true);
});

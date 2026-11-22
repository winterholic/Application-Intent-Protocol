// 외부 저장소(adopt-external) create는 권한(allow)을 INSERT 전에 판정해야 한다. 권한 없는 호출자가 유일 제약에 걸리는 값을 보냈을 때
// ALREADY_EXISTS가 나오면(INSERT가 먼저 실행된 것) 호출자가 읽을 수 없는 행의 값을 한 값씩 확인할 수 있다.
// 환경: AIP_BIN, AIP_TEST_DATABASE_URL(libpq key=value), 선택 AIP_PROBE_PG_CONTAINER(없으면 호스트 psql). 근거: plan-docs/hardening-1.0/policy.md D13.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { setup } from '../../plan-docs/hardening-1.0/policy-probes/h.mjs';

const SOURCE = `DROP SCHEMA IF EXISTS ext_order CASCADE; CREATE SCHEMA ext_order;
CREATE TABLE ext_order.members(member_id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY, org int NOT NULL DEFAULT 1);
CREATE TABLE ext_order.claims(claim_id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY, member_id bigint NOT NULL REFERENCES ext_order.members(member_id), tag text NOT NULL UNIQUE, status text NOT NULL DEFAULT 'open');
INSERT INTO ext_order.members(org) VALUES (1),(1);
INSERT INTO ext_order.claims(member_id,tag,status) VALUES (1,'secret1','open');`;
const DEFINITION = `actor Members
resource Members { fields { id: Id; org: Int default database } rows read when true
  expose read { select id, org; sort id; budget { rows 20; depth 1; deadline 2s; cost 1000 } } }
resource Claims {
  fields { id: Id; member: Members; tag: Text; status: Text default database }
  rows read when member = actor
  expose read { select id, tag, status; sort id; budget { rows 20; depth 1; deadline 2s; cost 1000 } }
  expose create { allow member = actor; fields member, tag }
}`;

test('an external create the caller may not make answers FORBIDDEN whether or not the unique value exists', { timeout: 120000 }, async () => {
  const s = await setup('ext_create_order', DEFINITION, '', { u1: 1, u2: 2 }, { external: { sourceSchema: 'ext_order', sql: SOURCE } });
  try {
    let n = 0;
    const create = async (member, tag) => (await s.call('u2', '/apply', { key: `order-${++n}-${Date.now()}`, request: { atomic: [{ create: 'Claims', values: { member, tag } }] } })).body;
    // u2는 Members 2이고 Members 1의 claim을 만들 권한이 없다. 'secret1'은 Members 1의 숨은 claim 값이다.
    const existing = await create('1', 'secret1');
    const fresh = await create('1', 'fresh1');
    assert.equal(existing.code, 'WIRE.FORBIDDEN', JSON.stringify(existing));
    assert.deepEqual(existing, fresh, 'the refusal must not depend on whether the unique value exists');
    // 권한이 있는 호출자의 유일 제약 충돌은 그대로 ALREADY_EXISTS다(공개 계약 불변).
    assert.equal((await create('2', 'secret1')).code, 'WIRE.ALREADY_EXISTS');
    assert.equal((await create('2', 'fresh2')).ok, true);
  } finally {
    s.stop();
  }
});

// K4(policy.md): allow가 생략한 `default database` 필드(status)도 읽는 외부 create. 그 필드의 값은 INSERT 뒤에야 알 수 있어 allow 전체는
// INSERT 뒤에 판정하지만, 그 필드를 읽지 않는 AND 항(`member = actor`)은 INSERT 전에 판정한다. 아니면 권한 없는 호출자가
// ALREADY_EXISTS와 FORBIDDEN의 차이로 숨은 행의 값을 확인한다.
const K4_SOURCE = (status) => `DROP SCHEMA IF EXISTS ext_order_k4 CASCADE; CREATE SCHEMA ext_order_k4;
CREATE TABLE ext_order_k4.members(member_id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY, org int NOT NULL DEFAULT 1);
CREATE TABLE ext_order_k4.claims(claim_id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY, member_id bigint NOT NULL REFERENCES ext_order_k4.members(member_id), tag text NOT NULL UNIQUE, status text NOT NULL DEFAULT '${status}');
INSERT INTO ext_order_k4.members(org) VALUES (1),(1);
INSERT INTO ext_order_k4.claims(member_id,tag,status) VALUES (1,'secret1','open');`;
const K4_DEFINITION = `actor Members
resource Members { fields { id: Id; org: Int default database } rows read when true
  expose read { select id, org; sort id; budget { rows 20; depth 1; deadline 2s; cost 1000 } } }
resource Claims {
  fields { id: Id; member: Members; tag: Text; status: Text default database }
  rows read when member = actor
  expose read { select id, tag, status; sort id; budget { rows 20; depth 1; deadline 2s; cost 1000 } }
  expose create { allow member = actor and status = "open"; fields member, tag }
}`;

test('an external create whose allow also reads a database default refuses the caller who fails the other terms before the INSERT', { timeout: 120000 }, async () => {
  const s = await setup('ext_create_k4', K4_DEFINITION, '', { u1: 1, u2: 2 }, { external: { sourceSchema: 'ext_order_k4', sql: K4_SOURCE('open') } });
  try {
    let n = 0;
    const create = async (member, tag) => (await s.call('u2', '/apply', { key: `k4-${++n}-${Date.now()}`, request: { atomic: [{ create: 'Claims', values: { member, tag } }] } })).body;
    // u2는 Members 2다. `member = actor`를 못 채우므로 status 기본값과 무관하게 FORBIDDEN이고, 숨은 값 secret1과 새 값이 같은 응답이다.
    const existing = await create('1', 'secret1');
    const fresh = await create('1', 'fresh1');
    assert.equal(existing.code, 'WIRE.FORBIDDEN', JSON.stringify(existing));
    assert.deepEqual(existing, fresh, 'the refusal must not depend on whether the unique value exists');
    // 모든 항을 채우는 호출자는 그대로다(공개 계약 불변): 유일 충돌은 ALREADY_EXISTS, 새 값은 성공.
    assert.equal((await create('2', 'secret1')).code, 'WIRE.ALREADY_EXISTS');
    assert.equal((await create('2', 'fresh2')).ok, true);
  } finally {
    s.stop();
  }
});

test('K4 residual: a caller who passes every term except the database default still sees the unique conflict first (owner decision)', { timeout: 120000 }, async () => {
  // DB 기본값이 'closed'라 `status = "open"`이 거짓이다. `member = actor`는 채우므로 INSERT까지 가고, 그 뒤에야 거절된다.
  // 이 줄은 남은 노출을 고정하는 것이다(decisions/a.md K4). 선택지 (a) 정의 거부가 확정되면 정의가 거부돼 이 시험이 바뀐다.
  const s = await setup('ext_create_k4r', K4_DEFINITION, '', { u1: 1, u2: 2 }, { external: { sourceSchema: 'ext_order_k4', sql: K4_SOURCE('closed') } });
  try {
    let n = 0;
    const create = async (member, tag) => (await s.call('u2', '/apply', { key: `k4r-${++n}-${Date.now()}`, request: { atomic: [{ create: 'Claims', values: { member, tag } }] } })).body;
    assert.equal((await create('2', 'secret1')).code, 'WIRE.ALREADY_EXISTS', 'residual: unique conflict before the default-dependent term is judged');
    assert.equal((await create('2', 'fresh2')).code, 'WIRE.FORBIDDEN');
  } finally {
    s.stop();
  }
});

// A-05: 확장 `ctx.data.create`를 외부 저장소 위에서 돌린다(policy.md 4절 "미실행"이던 것). create_row를 공유하므로 같은 판정 순서를 따라야 한다.
const EXT_DEFINITION = (allow) => `actor Members
resource Members { fields { id: Id; org: Int default database } rows read when true
  expose read { select id, org; sort id; budget { rows 20; depth 1; deadline 2s; cost 1000 } } }
resource Claims {
  fields { id: Id; member: Members; tag: Text; status: Text default database }
  rows read when member = actor
  expose read { select id, tag, status; sort id; budget { rows 20; depth 1; deadline 2s; cost 1000 } }
  expose create { allow ${allow}; fields member, tag }
  extension write claimit { input { member: Members.Id; tag: Text(1..9) } output { id: Text(0..200) } access create.Claims effect db deadline 3s implementation "x.claimit" }
}`;
const EXT_WORKER = `export async function claimit(input, ctx) { const id = await ctx.data.create('Claims', { member: input.member, tag: input.tag }); return { id: String(id) }; }`;

for (const [name, allow] of [['plain allow', 'member = actor'], ['allow that also reads a database default', 'member = actor and status = "open"']]) {
  test(`an extension ctx.data.create on external storage answers a forbidden caller the same whether or not the unique value exists (${name})`, { timeout: 120000 }, async () => {
    const s = await setup('ext_create_ext', EXT_DEFINITION(allow), '', { u1: 1, u2: 2 }, { workers: true, mjs: EXT_WORKER, external: { sourceSchema: 'ext_order_k4', sql: K4_SOURCE('open') } });
    try {
      let n = 0;
      const call = async (member, tag) => (await s.call('u2', '/apply', { key: `ext-${++n}-${Date.now()}`, request: { extension: 'Claims.claimit', input: { member, tag } } })).body;
      const existing = await call('1', 'secret1');
      const fresh = await call('1', 'fresh1');
      assert.equal(existing.ok, false, JSON.stringify(existing));
      assert.deepEqual(existing, fresh, 'the extension refusal must not depend on whether the unique value exists');
      // 권한 있는 호출자의 진단은 유지된다.
      assert.equal((await call('2', 'secret1')).code, 'WIRE.ALREADY_EXISTS');
      assert.equal((await call('2', 'fresh2')).ok, true);
    } finally {
      s.stop();
    }
  });
}

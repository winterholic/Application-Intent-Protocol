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

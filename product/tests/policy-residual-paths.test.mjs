// A2/A-05(stabilization Track A): 권한 판정 전에 DB 제약 오류가 호출자에게 나가는 경로가 있는지, 그리고 알려진 잔여 노출(K1~K3)의 현재 값.
// 권한이 없는 호출자는 충돌하는 값이든 아니든 같은 `WIRE.FORBIDDEN`을 받아야 하고, 권한이 있는 호출자의 진단(ALREADY_EXISTS, CHECK_FAILED 등)은 그대로여야 한다.
// 환경: AIP_BIN, AIP_TEST_DATABASE_URL(libpq key=value), 선택 AIP_PROBE_PG_CONTAINER. 근거: plan-docs/hardening-1.0/policy.md, plan-docs/stabilization-20261122/track-a.md.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { setup } from '../../plan-docs/hardening-1.0/policy-probes/h.mjs';

let seq = 0;
const key = () => `pr${++seq}-${Date.now()}`;
const apply = (s, who, request) => s.call(who, '/apply', { key: key(), request });
const code = (r) => (r.body.ok === false ? r.body.code : 'ok');
const R = '{ select id; sort id; budget { rows 20; depth 1; deadline 2s; cost 1000 } }';

const TRANSITIONS = `actor Member
resource Member { fields { id: Id; role: Int } rows read when true expose read ${R} }
resource Doc {
  fields { id: Id; owner: Member; code: Int; qty: Int; title: Text(1..9) }
  rows read when true
  unique code
  check small when qty <= 10
  transition recode { allow owner = actor; from true; patch code }
  transition setq { allow owner = actor; from true; patch qty }
  transition bump { allow owner = actor; from true; to code = code + 1 }
  transition rm { allow owner = actor; from true; delete }
  expose apply recode { target id; bulk maxRows 1 }
  expose apply setq { target id; bulk maxRows 1 }
  expose apply bump { target id; bulk maxRows 1 }
  expose apply rm { target id; bulk maxRows 1 }
  expose read ${R}
}
resource Child {
  fields { id: Id; parent: Doc; owner: Member }
  rows read when owner = actor
  expose read ${R}
}`;
const TRANSITIONS_SEED = `INSERT INTO $S.member(id,role) VALUES (1,1),(2,1);
INSERT INTO $S.doc(id,owner_id,code,qty,title) VALUES (1,1,100,1,'a'),(2,1,101,1,'b'),(3,2,300,1,'c'),(4,1,500,1,'d');
INSERT INTO $S.child(id,parent_id,owner_id) VALUES (1,4,1);
SELECT setval(pg_get_serial_sequence('$S.doc','id'), 500)`;

test('a transition the caller may not make answers FORBIDDEN whatever unique, check or reference constraint the values would hit', { timeout: 120000 }, async () => {
  const s = await setup('pr_transitions', TRANSITIONS, TRANSITIONS_SEED, { u1: 1, u2: 2 });
  try {
    const go = (name, id, values) => apply(s, 'u2', { apply: `Doc.${name}`, target: { ids: [String(id)] }, ...(values ? { values } : {}) });
    // u2는 Member 2다. Doc 1·2·4는 Member 1의 것이라 allow(owner = actor)가 거짓이다. Doc 3(code 300)은 u2의 것.
    // 유일 제약: 숨은 값처럼 보이는 300(다른 행의 값)과 새 값 999.
    assert.deepEqual(await go('recode', 1, { code: 300 }).then((r) => r.body), await go('recode', 1, { code: 999 }).then((r) => r.body), 'unique conflict must not change the refusal');
    assert.equal(code(await go('recode', 1, { code: 300 })), 'WIRE.FORBIDDEN');
    // check: 위반 값과 통과 값.
    assert.equal(code(await go('setq', 1, { qty: 99 })), 'WIRE.FORBIDDEN');
    assert.equal(code(await go('setq', 1, { qty: 5 })), 'WIRE.FORBIDDEN');
    // `to code = code + 1`이 Doc 2(101)와 충돌하는 Doc 1(100)과 충돌하지 않는 Doc 2(101 -> 102).
    assert.equal(code(await go('bump', 1)), 'WIRE.FORBIDDEN');
    assert.equal(code(await go('bump', 2)), 'WIRE.FORBIDDEN');
    // 참조 무결성: u2가 못 읽는 자식(Child 1)이 있는 부모 4와 자식이 없는 부모 1.
    assert.equal(code(await go('rm', 4)), 'WIRE.FORBIDDEN');
    assert.equal(code(await go('rm', 1)), 'WIRE.FORBIDDEN');
    // 권한 있는 호출자의 진단은 그대로다.
    assert.equal(code(await go('recode', 3, { code: 100 })), 'WIRE.ALREADY_EXISTS');
    assert.equal(code(await go('setq', 3, { qty: 99 })), 'WIRE.CHECK_FAILED');
    assert.equal(code(await go('recode', 3, { code: 998 })), 'ok');
  } finally {
    s.stop();
  }
});

const LIMITS = `actor Member
resource Member { fields { id: Id; role: Int } rows read when true expose read ${R} }
limit atMostOneDone on Item = atMost 1 where done = true
resource Item {
  fields { id: Id; owner: Member; done: Bool; qty: Int(0..100); note: Text(1..9) }
  rows read when owner = actor
  check small when qty <= 10
  invariant atMostOneDone per owner
  transition finish { allow owner = actor; from true; to done = true }
  expose apply finish { target id; bulk maxRows 1 }
  expose create { allow owner = actor; fields owner, done, qty, note }
  expose read ${R}
}`;
const LIMITS_SEED = `INSERT INTO $S.member(id,role) VALUES (1,1),(2,1);
INSERT INTO $S.item(id,owner_id,done,qty,note) VALUES (1,1,true,1,'a'),(2,1,false,1,'b'),(3,2,false,1,'c');
SELECT setval(pg_get_serial_sequence('$S.item','id'), 500)`;

test('a create the caller may not make answers FORBIDDEN whether or not a limit, check or hidden row would conflict', { timeout: 120000 }, async () => {
  const s = await setup('pr_limits', LIMITS, LIMITS_SEED, { u1: 1, u2: 2 });
  try {
    const create = (owner, values) => apply(s, 'u2', { atomic: [{ create: 'Item', values: { owner, ...values } }] });
    // Member 1은 이미 done 행이 있다(정원 1). u2는 Member 1의 행을 만들 권한이 없다.
    const hit = await create('1', { done: true, qty: 1, note: 'x' });
    const free = await create('1', { done: false, qty: 1, note: 'x' });
    const bad = await create('1', { done: false, qty: 99, note: 'x' });
    assert.equal(code(hit), 'WIRE.FORBIDDEN');
    assert.deepEqual(hit.body, free.body, 'a full limit must not change the refusal');
    assert.deepEqual(hit.body, bad.body, 'a failing check must not change the refusal');
    // u2가 못 읽는 Member 1의 행을 완료 처리하려는 시도는 대상이 없다.
    assert.equal(code(await apply(s, 'u2', { apply: 'Item.finish', target: { ids: ['2'] } })), 'WIRE.MISSING_TARGET');
    // 권한 있는 호출자(owner 2)의 진단은 유지된다: 첫 done은 성공, 둘째는 정원 초과(ALREADY_EXISTS).
    assert.equal(code(await create('2', { done: true, qty: 1, note: 'y' })), 'ok');
    assert.equal(code(await create('2', { done: true, qty: 1, note: 'z' })), 'WIRE.ALREADY_EXISTS');
    assert.equal(code(await create('2', { done: false, qty: 99, note: 'w' })), 'WIRE.CHECK_FAILED');
  } finally {
    s.stop();
  }
});

// K1~K3: 권한이 있는 호출자 기준의 알려진 잔여 노출. 정의 단계 제한(확정하지 않음, decisions/a.md)이 정해질 때까지 현재 값을 고정한다.
const KNOWN = `actor Member
resource Member { fields { id: Id; role: Int } rows read when true expose read ${R} }
resource Doc {
  fields { id: Id; owner: Member; cap: Int; code: Int; risk: Int(0..100) }
  rows read when true
  field code read when owner = actor
  field risk read when owner = actor
  unique code
  transition bump { from true; allow actor != null; to risk = risk + 1 }
  expose apply bump { target id; bulk maxRows 1 }
  expose create { allow actor != null; fields owner, cap, code, risk }
  expose read { select id, cap, code, risk; sort id; budget { rows 20; depth 1; deadline 2s; cost 1000 } }
}
resource Parent {
  fields { id: Id; owner: Member; name: Text(1..9) }
  rows read when true
  transition drop { from true; allow actor != null; delete }
  expose apply drop { target id; bulk maxRows 1 }
  expose read { select id, name; sort id; budget { rows 20; depth 1; deadline 2s; cost 1000 } }
}
resource Child {
  fields { id: Id; parent: Parent; owner: Member }
  rows read when owner = actor
  expose read ${R}
}`;
const KNOWN_SEED = `INSERT INTO $S.member(id,role) VALUES (1,1),(2,1);
INSERT INTO $S.doc(id,owner_id,cap,code,risk) VALUES (1,1,0,4242,63);
INSERT INTO $S.parent(id,owner_id,name) VALUES (1,1,'kid-a'),(2,1,'plain');
INSERT INTO $S.child(id,parent_id,owner_id) VALUES (1,1,1);
SELECT setval(pg_get_serial_sequence('$S.doc','id'), 500)`;

test('K1-K3 residuals (authorized caller, owner decision): the current responses are pinned', { timeout: 120000 }, async () => {
  const s = await setup('pr_known', KNOWN, KNOWN_SEED, { u1: 1, u2: 2 });
  try {
    const mk = (codeValue) => apply(s, 'u2', { atomic: [{ create: 'Doc', values: { owner: '1', cap: 0, code: codeValue, risk: 0 } }] });
    // K1: `unique code`에 가려진 필드. u2는 code를 못 읽지만 값 존재를 한 값씩 확인한다.
    assert.equal(code(await mk(4242)), 'WIRE.ALREADY_EXISTS');
    assert.equal(code(await mk(1)), 'ok');
    // K2: 가려진 Int(0..100)에 리터럴 증가를 반복하면 BAD_VALUE까지의 성공 횟수가 100 - risk다(여기서는 37번).
    let ok = 0;
    for (let i = 0; i < 120; i++) {
      if (!(await apply(s, 'u2', { apply: 'Doc.bump', target: { ids: ['1'] } })).body.ok) break;
      ok++;
    }
    assert.equal(ok, 37, 'hidden risk 63 is recovered from the number of increments before BAD_VALUE');
    // K3: u2가 못 읽는 자식이 있는 부모의 삭제는 REFERENCE_CONFLICT, 자식 없는 부모는 성공.
    assert.equal(code(await apply(s, 'u2', { apply: 'Parent.drop', target: { ids: ['1'] } })), 'WIRE.REFERENCE_CONFLICT');
    assert.equal(code(await apply(s, 'u2', { apply: 'Parent.drop', target: { ids: ['2'] } })), 'ok');
  } finally {
    s.stop();
  }
});

// 1.0 정책 경화 반례 모음. 실제 `aip service init/serve`와 PostgreSQL 17로 각 정의를 띄워 호출자가 가려진 값을 복원하는지 본다.
// 사용: AIP_BIN=<aip 바이너리> AIP_TEST_DATABASE_URL='host=... port=... dbname=... user=...' AIP_PROBE_PG_CONTAINER=<PG17 컨테이너>
//       node plan-docs/hardening-1.0/policy-probes/policy-probes.mjs [시나리오 id ...]
// 수정 전 바이너리: 결함 시나리오(D*)가 `RECOVERED ...`를 출력한다. 수정 뒤: `REJECTED DEF.POLICY_FIELD_NOT_USABLE_IN_TRANSITION`로 정의 단계에서 막힌다.
// 대조군(C*)과 알려진 한계(K*)는 두 바이너리에서 같은 줄을 출력해야 한다.
import { setup } from './h.mjs';

const J = (x) => JSON.stringify(x);
let seq = 0;
const key = () => `p${++seq}-${Date.now()}`;
const code = (r) => (r.body.ok === false ? r.body.code : 'ok');
const apply = (s, who, request) => s.call(who, '/apply', { key: key(), request });
const read = (s, who, query) => s.call(who, '/read', { query });
const bundle = (s, who, atomic) => apply(s, who, { atomic });

const MEMBER = `actor Member
resource Member { fields { id: Id; org: Int; score: Int; role: Int } rows read when true
  field score read when role = 9
  expose read { select id, org, role; sort id; budget { rows 20; depth 1; deadline 2s; cost 1000 } } }
predicate big(d: Doc) = d.risk > 3
`;
const DOC = (extra, trans, exp) => `${MEMBER}
resource Doc {
  fields { id: Id; owner: Member; org: Int; cap: Int; flag: Bool?; risk: Int; title: Text(1..9) }
  rows read when true
  field risk read when owner = actor
  ${extra}
  ${trans}
  expose read { select id, owner, org, cap, flag, title, risk; filter id.eq; sort id; budget { rows 20; depth 1; deadline 2s; cost 1000 } }
  ${exp}
}`;
const SEED = `INSERT INTO $S.member(id,org,score,role) VALUES (1,1,5,1),(2,1,50,9);
INSERT INTO $S.doc(id,owner_id,org,cap,flag,risk,title) VALUES (1,1,1,100,NULL,7,'a'),(2,1,1,100,NULL,2,'b'),(3,2,1,0,NULL,0,'c'),(4,1,1,100,NULL,7,'d');
SELECT setval(pg_get_serial_sequence('$S.doc','id'), 500)`;
const APPLY_T = 'expose apply t { target id; bulk maxRows 1 }';
const readDoc = (s, who, id) => read(s, who, { read: 'Doc', select: ['id', 'flag', 'risk', 'cap'], filter: [{ field: 'id', op: 'eq', value: String(id) }] });

const scenarios = [
  {
    id: 'D1-from-predicate',
    claim: '`from big(this)`가 술어 안에서 가려진 risk를 읽는다. u2는 risk를 못 읽는다(doc1=7, doc2=2).',
    def: DOC('', 'transition t { from big(this); allow actor != null; to flag = true }', APPLY_T),
    seed: SEED,
    run: async (s) => `doc1(risk7)=${code(await apply(s, 'u2', { apply: 'Doc.t', target: { ids: ['1'] } }))} doc2(risk2)=${code(await apply(s, 'u2', { apply: 'Doc.t', target: { ids: ['2'] } }))}`,
  },
  {
    id: 'D2-to-predicate-copy',
    claim: '`to flag = big(this)`가 가려진 값에서 계산한 비트를 읽을 수 있는 flag에 복사한다.',
    def: DOC('', 'transition t { from true; allow actor != null; to flag = big(this) }', APPLY_T),
    seed: SEED,
    run: async (s) => {
      await apply(s, 'u2', { apply: 'Doc.t', target: { ids: ['1'] } });
      await apply(s, 'u2', { apply: 'Doc.t', target: { ids: ['2'] } });
      return `flag(doc1)=${J((await readDoc(s, 'u2', 1)).body.rows?.[0]?.flag)} flag(doc2)=${J((await readDoc(s, 'u2', 2)).body.rows?.[0]?.flag)}`;
    },
  },
  {
    id: 'D3-actor-hidden-input',
    claim: '`allow actor.score >= input.n`가 호출자 자신의 가려진 score(5)를 FORBIDDEN 경계로 내준다.',
    def: DOC('', 'transition t { input { n: Int(1..100) } from true; allow actor.score >= input.n; to flag = true }', APPLY_T),
    seed: SEED,
    run: async (s) => {
      let best = 0;
      for (const n of [1, 2, 3, 4, 5, 6, 7, 8]) if ((await apply(s, 'u1', { apply: 'Doc.t', target: { ids: ['3'] }, input: { n } })).body.ok) best = n;
      return `largest n that succeeds = ${best}`;
    },
  },
  {
    id: 'D4-check-hidden-patch',
    claim: '`check c when risk <= cap`과 `patch cap`이 가려진 risk(doc1=7)를 CHECK_FAILED 경계로 내준다.',
    def: DOC('check c when risk <= cap', 'transition t { from true; allow actor != null; patch cap }', APPLY_T),
    seed: SEED,
    run: async (s) => {
      const out = [];
      for (const n of [1, 6, 7, 50]) out.push(`${n}:${code(await apply(s, 'u2', { apply: 'Doc.t', target: { ids: ['1'] }, values: { cap: n } }))}`);
      return out.join(' ');
    },
  },
  {
    id: 'D5-samescope-hidden',
    claim: '`sameScope risk`: 같은 risk(doc1=doc4=7)면 성공, 다르면 NOT_SAME_SCOPE.',
    def: DOC('', 'transition t { from true; allow actor != null; to flag = true }', 'expose apply t { target id; bulk maxRows 5; sameScope risk }'),
    seed: SEED,
    run: async (s) => `[1,4]=${code(await apply(s, 'u2', { apply: 'Doc.t', target: { ids: ['1', '4'] } }))} [1,2]=${code(await apply(s, 'u2', { apply: 'Doc.t', target: { ids: ['1', '2'] } }))}`,
  },
  {
    id: 'D6-aggregate-input-hidden',
    claim: '단독 집계 `where secret >= input.n`: n을 바꿔 남의 가려진 secret(37, 5)을 센다.',
    def: `actor Member
resource Member { fields { id: Id; org: Int } rows read when true expose read { select id, org; sort id; budget { rows 20; depth 1; deadline 2s; cost 1000 } } }
access all(a: Member) = true
resource Vote {
  fields { id: Id; owner: Member; amount: Int; secret: Int }
  rows read when owner = actor
  field secret read when owner = actor
  aggregate cnt: Int { input { n: Int }; source Vote; sourceAccess all(actor); where secret >= input.n; callerFilter none; rowOutput none; release count }
  expose read { select id, amount; sort id; budget { rows 20; depth 1; deadline 2s; cost 1000 } }
  expose aggregate cnt
}`,
    seed: `INSERT INTO $S.member(id,org) VALUES (1,1),(2,1); INSERT INTO $S.vote(id,owner_id,amount,secret) VALUES (1,1,1,37),(2,1,1,5)`,
    run: async (s) => {
      const out = [];
      for (const n of [0, 6, 37, 38]) out.push(`>=${n}:${J((await read(s, 'u2', { aggregate: 'Vote.cnt', input: { n } })).body.rows)}`);
      return `u2 sees count ${out.join(' ')}`;
    },
  },
  {
    id: 'D7-create-allow-predicate',
    claim: '`expose create allow riskAbove(cap)`: 호출자가 고른 cap과 남의 가려진 risk(37)를 견준다.',
    def: `actor Member
resource Member { fields { id: Id; org: Int } rows read when true expose read { select id, org; sort id; budget { rows 20; depth 1; deadline 2s; cost 1000 } } }
predicate riskAbove(n: Int) = exists Doc where org = 1 and risk >= n
resource Doc {
  fields { id: Id; owner: Member; org: Int; cap: Int; risk: Int }
  rows read when owner = actor
  field risk read when owner = actor
  expose read { select id, org, cap; sort id; budget { rows 20; depth 1; deadline 2s; cost 1000 } }
  expose create { allow riskAbove(cap) and owner = actor; fields owner, org, cap, risk }
}`,
    seed: `INSERT INTO $S.member(id,org) VALUES (1,1),(2,1); INSERT INTO $S.doc(id,owner_id,org,cap,risk) VALUES (1,1,1,0,37); SELECT setval(pg_get_serial_sequence('$S.doc','id'), 500)`,
    run: async (s) => {
      const out = [];
      for (const n of [0, 37, 38]) out.push(`cap=${n}:${code(await bundle(s, 'u2', [{ create: 'Doc', values: { owner: '2', org: 1, cap: n, risk: 0 } }]))}`);
      return out.join(' ');
    },
  },
  {
    id: 'D8-create-allow-ref-hop',
    claim: '`expose create allow owner.score >= cap`: 참조 이동 뒤 가려진 Member.score(37).',
    def: `actor Member
resource Member { fields { id: Id; role: Int; score: Int } rows read when true
  field score read when role = 9
  expose read { select id, role; sort id; budget { rows 20; depth 1; deadline 2s; cost 1000 } } }
resource Doc {
  fields { id: Id; owner: Member; cap: Int }
  rows read when true
  expose read { select id, cap; sort id; budget { rows 20; depth 1; deadline 2s; cost 1000 } }
  expose create { allow owner.score >= cap; fields owner, cap }
}`,
    seed: `INSERT INTO $S.member(id,role,score) VALUES (1,1,37),(2,1,0); SELECT setval(pg_get_serial_sequence('$S.doc','id'), 500)`,
    run: async (s) => {
      const out = [];
      for (const n of [0, 37, 38]) out.push(`cap=${n}:${code(await bundle(s, 'u2', [{ create: 'Doc', values: { owner: '1', cap: n } }]))}`);
      return out.join(' ');
    },
  },
  {
    id: 'D9-compose-copy',
    claim: 'compose `create OrderItem from this.sku`가 가려진 Cart.sku를 읽을 수 있는 OrderItem.sku로 복사한다.',
    def: `actor Member
resource Member { fields { id: Id; org: Int } rows read when true expose read { select id; sort id; budget { rows 20; depth 1; deadline 1s; cost 100 } } }
resource Cart { fields { id: Id; owner: Member; sku: Text(1..40); qty: Int(1..100); ordered: Bool }
  rows read when owner = actor
  field sku read when ordered = true
  transition checkout { allow owner = actor; from ordered = false; to ordered = true }
  expose read { select id, sku, qty, ordered; sort id; budget { rows 20; depth 1; deadline 1s; cost 100 } }
  expose compose { bulk maxRows 20; sameScope owner; transitions checkout; create OrderItem from this.owner, this.sku, this.qty }
}
resource OrderItem { fields { id: Id; owner: Member; sku: Text(1..40); qty: Int(1..100) }
  rows read when owner = actor
  expose read { select id, sku, qty; sort id; budget { rows 20; depth 1; deadline 1s; cost 100 } }
  expose create { allow owner = actor; fields owner, sku, qty } }`,
    seed: `INSERT INTO $S.member(id,org) VALUES (1,1); INSERT INTO $S.cart(id,owner_id,sku,qty,ordered) VALUES (1,1,'SECRET-SKU-9',2,false)`,
    run: async (s) => {
      await apply(s, 'u1', { compose: 'Cart', targets: { ids: ['1'] }, steps: [{ transition: 'checkout' }, { create: 'OrderItem', values: { owner: { item: 'owner' }, sku: { item: 'sku' }, qty: { item: 'qty' } } }] });
      return `cart.sku via read=${J((await read(s, 'u1', { read: 'Cart', select: ['id', 'sku'] })).body.rows?.[0]?.sku)} orderItems=${J((await read(s, 'u1', { read: 'OrderItem', select: ['id', 'sku'] })).body.rows)}`;
    },
  },
  {
    id: 'D10-effect-copy',
    claim: '효과 `create Log { val = this.risk }`가 가려진 risk(37)를 읽을 수 있는 Log.val에 복사한다.',
    def: `actor Member
resource Member { fields { id: Id; org: Int } rows read when true expose read { select id, org; sort id; budget { rows 20; depth 1; deadline 2s; cost 1000 } } }
resource Log { fields { id: Id; owner: Member; val: Int; flag: Bool? } rows read when true
  expose read { select id, val, flag; sort id; budget { rows 20; depth 1; deadline 2s; cost 1000 } } }
resource Doc {
  fields { id: Id; owner: Member; risk: Int; flag: Bool? }
  rows read when true
  field risk read when owner = actor
  transition t { from true; allow actor != null; to flag = true
    create Log { owner = this.owner; val = this.risk } }
  expose apply t { target id; bulk maxRows 1 }
  expose read { select id, flag, risk; sort id; budget { rows 20; depth 1; deadline 2s; cost 1000 } }
}`,
    seed: `INSERT INTO $S.member(id,org) VALUES (1,1),(2,1); INSERT INTO $S.doc(id,owner_id,risk,flag) VALUES (1,1,37,NULL)`,
    run: async (s) => {
      await apply(s, 'u2', { apply: 'Doc.t', target: { ids: ['1'] } });
      return `u2 reads Log=${J((await read(s, 'u2', { read: 'Log', select: ['id', 'val'] })).body.rows)}`;
    },
  },
  {
    id: 'D11-access-guard',
    claim: '`sourceAccess guardA(actor, input.n)`(`a.score >= n`)가 자기 행의 가려진 score(5)를 ACCESS_DENIED 경계로 내준다.',
    def: `actor Member
resource Member { fields { id: Id; role: Int; score: Int } rows read when true field score read when role = 9
  expose read { select id, role; sort id; budget { rows 20; depth 1; deadline 2s; cost 1000 } } }
access guardA(a: Member, n: Int) = a.score >= n
resource Doc {
  fields { id: Id; owner: Member; risk: Int; title: Text(1..9) }
  rows read when owner = actor
  aggregate c: Int { input { n: Int }; source Doc; sourceAccess guardA(actor, input.n); where owner = actor; callerFilter none; rowOutput none; release count }
  expose read { select id, title; sort id; budget { rows 20; depth 1; deadline 2s; cost 1000 } }
  expose aggregate c
}`,
    seed: `INSERT INTO $S.member(id,role,score) VALUES (1,1,5); INSERT INTO $S.doc(id,owner_id,risk,title) VALUES (1,1,1,'a')`,
    run: async (s) => {
      let best = 0;
      for (const n of [1, 2, 3, 4, 5, 6, 7, 8]) if ((await read(s, 'u1', { aggregate: 'Doc.c', input: { n } })).body.ok) best = n;
      return `largest n that passes the guard = ${best}`;
    },
  },
  {
    id: 'D12-operation-allow',
    claim: '`operation read probe { allow riskAbove(input.n) }`가 읽을 수 없는 행의 가려진 risk(37)를 ACCESS_DENIED 경계로 내준다.',
    def: `actor Member
resource Member { fields { id: Id; role: Int } rows read when true expose read { select id, role; sort id; budget { rows 20; depth 1; deadline 2s; cost 1000 } } }
predicate riskAbove(n: Int) = exists Doc where org = 1 and risk >= n
resource Doc {
  fields { id: Id; owner: Member; org: Int; risk: Int }
  rows read when owner = actor
  field risk read when owner = actor
  expose read { select id, org; sort id; budget { rows 20; depth 1; deadline 2s; cost 1000 } }
}
operation read probe { input { n: Int } output { ok: Bool } allow riskAbove(input.n) effect none deadline 2s implementation "x.probe" }`,
    seed: `INSERT INTO $S.member(id,role) VALUES (1,1),(2,1); INSERT INTO $S.doc(id,owner_id,org,risk) VALUES (1,1,1,37)`,
    workers: 'export async function probe(input, ctx) { return { ok: true }; }',
    run: async (s) => {
      const out = [];
      for (const n of [0, 37, 38]) out.push(`n=${n}:${code(await s.call('u2', '/operation', { operation: 'probe', input: { n } }))}`);
      return out.join(' ');
    },
  },
  // 대조군: 정상 동작이 유지되는지.
  {
    id: 'C1-tenant-existence',
    claim: '다른 테넌트의 id와 없는 id는 create 참조, apply 대상, filter, patch 참조, 섞인 대상 목록에서 같은 응답이다.',
    def: `actor Member
resource Member { fields { id: Id; org: Int; name: Text(1..20) } rows read when org = actor.org
  expose read { select id, org, name; filter id.eq; sort id; budget { rows 20; depth 1; deadline 2s; cost 1000 } } }
resource Note {
  fields { id: Id; org: Int; owner: Member; body: Text(1..20) }
  rows read when org = actor.org
  transition edit { allow owner = actor or org = actor.org; from true; patch body }
  transition reown { allow actor != null; from true; patch owner }
  expose create { allow org = actor.org; fields org, owner, body }
  expose apply edit { target id; bulk maxRows 5; sameScope owner }
  expose apply reown { target id; bulk maxRows 1 }
  expose read { select id, org, owner, body; filter id.eq, owner.eq; sort id; budget { rows 20; depth 1; deadline 2s; cost 1000 } }
}
resource Comment {
  fields { id: Id; note: Note; author: Member; text: Text(1..20) }
  rows read when author = actor
  expose create { allow author = actor; fields note, author, text }
  expose read { select id, note, text; filter id.eq; sort id; budget { rows 20; depth 1; deadline 2s; cost 1000 } }
}`,
    seed: `INSERT INTO $S.member(id,org,name) VALUES (1,1,'a1'),(2,1,'a2'),(10,2,'b1');
INSERT INTO $S.note(id,org,owner_id,body) VALUES (1,1,1,'n1'),(2,1,2,'n2'),(20,2,10,'other'),(21,2,10,'other2'); SELECT setval(pg_get_serial_sequence('$S.note','id'), 500)`,
    run: async (s) => {
      const pairs = [
        ['create ref', await bundle(s, 'u1', [{ create: 'Comment', values: { note: '20', author: '1', text: 'x' } }]), await bundle(s, 'u1', [{ create: 'Comment', values: { note: '999', author: '1', text: 'x' } }])],
        ['apply', await apply(s, 'u1', { apply: 'Note.edit', target: { ids: ['20'] }, values: { body: 'z' } }), await apply(s, 'u1', { apply: 'Note.edit', target: { ids: ['999'] }, values: { body: 'z' } })],
        ['mixed targets', await apply(s, 'u1', { apply: 'Note.edit', target: { ids: ['1', '20'] }, values: { body: 'z' } }), await apply(s, 'u1', { apply: 'Note.edit', target: { ids: ['1', '999'] }, values: { body: 'z' } })],
        ['filter owner.eq', await read(s, 'u1', { read: 'Note', select: ['id'], filter: [{ field: 'owner', op: 'eq', value: '10' }] }), await read(s, 'u1', { read: 'Note', select: ['id'], filter: [{ field: 'owner', op: 'eq', value: '999' }] })],
        ['note owner', await bundle(s, 'u1', [{ create: 'Note', values: { org: 1, owner: '10', body: 'x' } }]), await bundle(s, 'u1', [{ create: 'Note', values: { org: 1, owner: '999', body: 'x' } }])],
        ['patch ref', await apply(s, 'u1', { apply: 'Note.reown', target: { ids: ['1'] }, values: { owner: '10' } }), await apply(s, 'u1', { apply: 'Note.reown', target: { ids: ['1'] }, values: { owner: '999' } })],
      ];
      return pairs.map(([n, a, b]) => `${n}=${J(a.body.rows ?? a.body) === J(b.body.rows ?? b.body) || (a.body.code === b.body.code && a.body.msg === b.body.msg) ? 'same' : 'DIFF'}`).join(' ');
    },
  },
  {
    id: 'C2-bundle-created-refs',
    claim: 'bundle의 `refs`·`target.created`로 호출자가 읽을 수 없게 만든 행을 가리키면 MISSING_TARGET이다.',
    def: `actor Member
resource Member { fields { id: Id; org: Int } rows read when org = actor.org }
resource Note {
  fields { id: Id; org: Int; owner: Member; body: Text(1..20) }
  rows read when org = actor.org
  transition edit { allow actor != null; from true; patch body }
  expose create { allow actor != null; fields org, owner, body }
  expose apply edit { target id; bulk maxRows 5 }
  expose read { select id, org, body; filter id.eq; sort id; budget { rows 20; depth 1; deadline 2s; cost 1000 } }
}
resource Reply {
  fields { id: Id; note: Note; text: Text(1..20) }
  rows read when note.org = actor.org
  expose create { allow actor != null; fields note, text }
  expose read { select id, text; sort id; budget { rows 20; depth 1; deadline 2s; cost 1000 } }
}`,
    seed: `INSERT INTO $S.member(id,org) VALUES (1,1),(10,2); INSERT INTO $S.note(id,org,owner_id,body) VALUES (1,1,1,'mine'),(20,2,10,'foreign'); SELECT setval(pg_get_serial_sequence('$S.note','id'), 500)`,
    run: async (s) =>
      `refs=${code(await bundle(s, 'u1', [{ create: 'Note', values: { org: 2, owner: '1', body: 'x' } }, { create: 'Reply', values: { text: 'hi' }, refs: { note: { created: 0 } } }]))} target.created=${code(await bundle(s, 'u1', [{ create: 'Note', values: { org: 2, owner: '1', body: 'x' } }, { apply: 'Note.edit', target: { created: 0 }, values: { body: 'changed' } }]))}`,
  },
  {
    id: 'C3-extension-ctx',
    claim: '확장 ctx.data는 호출자 정책을 따른다: 가려진 필드 null, 가려진 필드 filter 거부, 다른 테넌트 행 없음, 선언 안 한 읽기·internal 전이 거부.',
    def: `actor Member
resource Member { fields { id: Id; org: Int } rows read when org = actor.org
  expose read { select id, org; filter id.eq; sort id; budget { rows 20; depth 1; deadline 2s; cost 1000 } } }
resource Doc {
  fields { id: Id; owner: Member; org: Int; risk: Int; flag: Bool? }
  rows read when org = actor.org
  field risk read when owner = actor
  transition t { from true; allow owner = actor; to flag = true }
  transition internalOnly { from true; allow owner = actor; to flag = false }
  expose apply t { target id; bulk maxRows 1 }
  expose apply internalOnly { target id; bulk maxRows 1; internal }
  expose read { select id, org, flag, risk; filter id.eq, org.eq; sort id; budget { rows 20; depth 1; deadline 2s; cost 1000 } }
  extension write probe { input { which: Text(1..30); id: Doc.Id } output { out: Text(0..900) } access read.Doc, Doc.t effect db deadline 3s implementation "x.probe" }
}
resource Secret { fields { id: Id; v: Int } rows read when true
  expose read { select id, v; sort id; budget { rows 20; depth 1; deadline 2s; cost 1000 } } }`,
    seed: `INSERT INTO $S.member(id,org) VALUES (1,1),(2,1),(10,2);
INSERT INTO $S.doc(id,owner_id,org,risk,flag) VALUES (1,1,1,37,NULL),(2,2,1,5,NULL),(20,10,2,99,NULL); INSERT INTO $S.secret(id,v) VALUES (1,7)`,
    workers: `export async function probe(input, ctx) {
  const w = input.which; let out;
  if (w === 'readHidden') out = await ctx.data.read({ read: 'Doc', select: ['id','risk','org'], filter: [{ field: 'id', op: 'eq', value: input.id }] });
  else if (w === 'filterHidden') out = await ctx.data.read({ read: 'Doc', select: ['id'], filter: [{ field: 'risk', op: 'gte', value: 1 }] });
  else if (w === 'readForeign') out = await ctx.data.read({ read: 'Doc', select: ['id','org'], filter: [{ field: 'org', op: 'eq', value: 2 }] });
  else if (w === 'readNotGranted') out = await ctx.data.read({ read: 'Secret', select: ['id','v'] });
  else if (w === 'applyInternal') out = await ctx.data.apply('Doc', 'internalOnly', [input.id], undefined, undefined, undefined);
  else if (w === 'applyAllowed') out = await ctx.data.apply('Doc', 't', [input.id], undefined, undefined, undefined);
  return { out: JSON.stringify(out).slice(0, 800) };
}`,
    run: async (s) => {
      const out = [];
      for (const [which, id] of [['readHidden', '1'], ['filterHidden', '1'], ['readForeign', '1'], ['readNotGranted', '1'], ['applyInternal', '2'], ['applyAllowed', '1'], ['applyAllowed', '2']]) {
        const r = await apply(s, 'u2', { extension: 'Doc.probe', input: { which, id } });
        out.push(`${which}(${id})=${r.body.ok ? r.body.output.out.slice(0, 40) : r.body.code}`);
      }
      return out.join(' | ');
    },
  },
  {
    id: 'C4-internal-and-auth',
    claim: 'internal 전이는 apply·bundle·compose 어디서도 호출자가 못 부른다. 잘못된 토큰은 UNAUTHENTICATED.',
    def: `actor Member
resource Member { fields { id: Id; org: Int } rows read when true }
resource Doc {
  fields { id: Id; owner: Member; flag: Bool? }
  rows read when true
  transition pub { from true; allow actor != null; to flag = true }
  transition priv { from true; allow actor != null; to flag = false }
  expose apply pub { target id; bulk maxRows 1 }
  expose apply priv { target id; bulk maxRows 1; internal }
  expose read { select id, flag; sort id; budget { rows 20; depth 1; deadline 2s; cost 1000 } }
}`,
    seed: `INSERT INTO $S.member(id,org) VALUES (1,1); INSERT INTO $S.doc(id,owner_id,flag) VALUES (1,1,NULL)`,
    run: async (s) =>
      `apply=${code(await apply(s, 'u1', { apply: 'Doc.priv', target: { ids: ['1'] } }))} bundle=${code(await bundle(s, 'u1', [{ apply: 'Doc.priv', target: { ids: ['1'] } }]))} compose=${code(await apply(s, 'u1', { compose: 'Doc', targets: { ids: ['1'] }, steps: [{ transition: 'priv' }] }))} badToken=${code(await s.call(null, '/read', { query: { read: 'Doc', select: ['id'] } }))}`,
  },
  {
    id: 'C5-cursor-filter-sort',
    claim: 'after 커서에 가려진 필드 키를 넣으면 BAD_CURSOR, 가려진 필드 sort·filter는 계약 밖이다. 참조 filter는 대상 행 가시성을 따른다.',
    def: `actor Member
resource Member { fields { id: Id; org: Int } rows read when org = actor.org
  expose read { select id, org; filter id.eq; sort id; budget { rows 20; depth 1; deadline 2s; cost 1000 } } }
resource Doc {
  fields { id: Id; org: Int; owner: Member; title: Text(1..9); risk: Int }
  rows read when org = actor.org
  field risk read when owner = actor
  expose read { select id, title, owner; filter id.eq, title.eq, owner.eq; sort id, title; budget { rows 3; depth 1; deadline 2s; cost 1000; cursor; offset 10 } }
}`,
    seed: `INSERT INTO $S.member(id,org) VALUES (1,1),(10,2); INSERT INTO $S.doc(id,org,owner_id,title,risk) VALUES (1,1,1,'a',90),(2,1,10,'b',10),(3,1,1,'c',50)`,
    run: async (s) => {
      const q = (extra) => read(s, 'u1', { read: 'Doc', select: ['id', 'title'], ...extra });
      return [
        `after+risk=${code(await q({ sort: [{ field: 'id', dir: 'asc' }], after: { id: '1', risk: 60 } }))}`,
        `sort risk=${code(await q({ sort: [{ field: 'risk', dir: 'asc' }] }))}`,
        `filter risk=${code(await q({ filter: [{ field: 'risk', op: 'gte', value: 60 }] }))}`,
        `filter owner=foreign member rows=${J((await q({ filter: [{ field: 'owner', op: 'eq', value: '10' }] })).body.rows)}`,
      ].join(' ');
    },
  },
  {
    id: 'C6-idempotency-key-scope',
    claim: '같은 멱등 key를 다른 주체(다른 테넌트)가 같은 본문으로 보내도 저장된 결과를 재생받지 않는다. 다른 본문을 보내도 key 충돌이 드러나지 않는다.',
    def: `actor Member
resource Member { fields { id: Id; org: Int } rows read when org = actor.org }
resource Note {
  fields { id: Id; org: Int; owner: Member; body: Text(1..20) }
  rows read when org = actor.org
  transition edit { allow actor != null; from true; patch body }
  expose apply edit { target id; bulk maxRows 5 }
  expose read { select id, org, body; filter id.eq; sort id; budget { rows 20; depth 1; deadline 2s; cost 1000 } }
}`,
    seed: `INSERT INTO $S.member(id,org) VALUES (1,1),(10,2); INSERT INTO $S.note(id,org,owner_id,body) VALUES (1,1,1,'mine'),(20,2,10,'foreign')`,
    run: async (s) => {
      const k = `shared-${Date.now()}`;
      const body = { apply: 'Note.edit', target: { ids: ['1'] }, values: { body: 'viaU1' } };
      const first = await s.call('u1', '/apply', { key: k, request: body });
      const sameBody = await s.call('v1', '/apply', { key: k, request: body });
      const otherBody = await s.call('v1', '/apply', { key: k, request: { apply: 'Note.edit', target: { ids: ['20'] }, values: { body: 'diff' } } });
      return `u1=${code(first)} v1 same body=${code(sameBody)} (not a replay of u1's changed:[1]: ${J(sameBody.body.changed) !== J(first.body.changed)}) v1 other body=${code(otherBody)}`;
    },
  },
  {
    id: 'C7-read-extension-ctx-read',
    claim: '읽기 확장은 `access read.<Resource>` 선언 없이 `ctx.data.read`를 부를 수 있다(선언할 문법이 없음). 그래도 호출자의 행·필드 정책, 계약(select·filter·sort·limit), 노출 여부, 루트 질의 가능 여부가 공개 `/read`와 같이 적용된다.',
    def: `actor Member
resource Member { fields { id: Id; org: Int } rows read when org = actor.org
  expose read { select id, org; filter id.eq; sort id; budget { rows 20; depth 1; deadline 2s; cost 1000 } } }
resource Doc {
  fields { id: Id; owner: Member; org: Int; risk: Int; title: Text(1..9); memo: Text(1..9) }
  rows read when org = actor.org
  field risk read when owner = actor
  expose read { select id, org, title, risk, owner; filter id.eq, org.eq; sort id; budget { rows 2; depth 1; deadline 2s; cost 1000 } }
  extension read peek { input { which: Text(1..30); id: Doc.Id } output { out: Text(0..1800) } effect none deadline 3s implementation "x.peek" }
}
resource Note { fields { id: Id; doc: Doc; text: Text(1..9) } rows read when true
  expose read { select id, text; sort id } }
resource Secret { fields { id: Id; v: Int } rows read when true }`,
    seed: `INSERT INTO $S.member(id,org) VALUES (1,1),(2,1),(10,2);
INSERT INTO $S.doc(id,owner_id,org,risk,title,memo) VALUES (1,1,1,37,'a','m1'),(2,2,1,5,'b','m2'),(3,2,1,6,'c','m3'),(20,10,2,99,'foreign','m20');
INSERT INTO $S.note(id,doc_id,text) VALUES (1,1,'n1'); INSERT INTO $S.secret(id,v) VALUES (1,7)`,
    workers: `export async function peek(input, ctx) {
  const w = input.which; let out;
  const read = (q) => ctx.data.read(q);
  if (w === 'ran') out = 'ran';
  else if (w === 'readHidden') out = await read({ read: 'Doc', select: ['id','risk'], filter: [{ field: 'id', op: 'eq', value: input.id }] });
  else if (w === 'filterHidden') out = await read({ read: 'Doc', select: ['id'], filter: [{ field: 'risk', op: 'gte', value: 1 }] });
  else if (w === 'foreignTenant') out = await read({ read: 'Doc', select: ['id','org'], filter: [{ field: 'org', op: 'eq', value: 2 }] });
  else if (w === 'foreignById') out = await read({ read: 'Doc', select: ['id','org'], filter: [{ field: 'id', op: 'eq', value: '20' }] });
  else if (w === 'unexposed') out = await read({ read: 'Secret', select: ['id','v'] });
  else if (w === 'traverseOnly') out = await read({ read: 'Note', select: ['id','text'] });
  else if (w === 'notInSelect') out = await read({ read: 'Doc', select: ['id','memo'] });
  else if (w === 'overBudget') out = await read({ read: 'Doc', select: ['id'], limit: 1000 });
  else if (w === 'sortNotAllowed') out = await read({ read: 'Doc', select: ['id'], sort: [{ field: 'title', dir: 'asc' }] });
  else if (w === 'filterOpNotAllowed') out = await read({ read: 'Doc', select: ['id'], filter: [{ field: 'org', op: 'gte', value: 0 }] });
  else if (w === 'unknownKey') out = await read({ read: 'Doc', select: ['id'], where: 'true' });
  else if (w === 'publicSame') out = await read({ read: 'Doc', select: ['id','org','title','risk'], sort: [{ field: 'id', dir: 'asc' }] });
  return { out: JSON.stringify(out).slice(0, 1700) };
}`,
    run: async (s) => {
      const call = async (who, which, id = '1') => {
        const r = await s.call(who, '/extension', { extension: 'Doc.peek', input: { which, id } });
        return r.body.ok ? r.body.output.out : r.body.code;
      };
      const out = [];
      for (const which of ['readHidden', 'filterHidden', 'foreignTenant', 'foreignById', 'unexposed', 'traverseOnly', 'notInSelect', 'overBudget', 'sortNotAllowed', 'filterOpNotAllowed', 'unknownKey']) {
        out.push(`${which}=${(await call('u2', which)).slice(0, 60)}`);
      }
      const viaExt = JSON.parse(await call('u2', 'publicSame'));
      const viaPublic = (await read(s, 'u2', { read: 'Doc', select: ['id', 'org', 'title', 'risk'], sort: [{ field: 'id', dir: 'asc' }] })).body.rows;
      out.push(`publicSame=${J(viaExt.rows) === J(viaPublic)}`);
      const foreignId = await s.call('u2', '/extension', { extension: 'Doc.peek', input: { which: 'ran', id: '20' } });
      const missingId = await s.call('u2', '/extension', { extension: 'Doc.peek', input: { which: 'ran', id: '999' } });
      out.push(`foreignVsMissingInputId=${J(foreignId.body) === J(missingId.body)}`);
      return out.join(' | ');
    },
  },
  {
    id: 'C8-read-extension-other-ops',
    claim: '읽기 확장의 `ctx.data.aggregate`는 선언한 집계만, 확장 입력에 묶인 값으로만 부를 수 있다. apply·create·http·jobs는 읽기 확장에서 거부된다.',
    def: `actor Member
resource Member { fields { id: Id; org: Int } rows read when true }
access all(a: Member) = true
resource Doc {
  fields { id: Id; owner: Member; org: Int; title: Text(1..9) }
  rows read when owner = actor
  transition t { from true; allow owner = actor; to org = org }
  aggregate cnt: Int { input { n: Int }; source Doc; sourceAccess all(actor); where owner = actor and org >= input.n; callerFilter none; rowOutput none; release count }
  aggregate other: Int { input { n: Int }; source Doc; sourceAccess all(actor); where owner = actor and org >= input.n; callerFilter none; rowOutput none; release count }
  expose read { select id, title; sort id; budget { rows 5; depth 1; deadline 2s; cost 1000 } }
  expose apply t { target id; bulk maxRows 1 }
  expose aggregate cnt
  expose aggregate other
  extension read peek { input { which: Text(1..30); n: Int } output { out: Text(0..900) } access Doc.cnt effect none deadline 3s implementation "x.peek" }
}`,
    seed: `INSERT INTO $S.member(id,org) VALUES (1,1),(2,1); INSERT INTO $S.doc(id,owner_id,org,title) VALUES (1,1,3,'a'),(2,2,4,'b')`,
    workers: `export async function peek(input, ctx) {
  const w = input.which; let out;
  if (w === 'declared') out = await ctx.data.aggregate('Doc', 'cnt', { n: input.n });
  else if (w === 'rebound') out = await ctx.data.aggregate('Doc', 'cnt', { n: input.n + 100 });
  else if (w === 'undeclared') out = await ctx.data.aggregate('Doc', 'other', { n: input.n });
  else if (w === 'apply') out = await ctx.data.apply('Doc', 't', ['1']);
  else if (w === 'create') out = await ctx.data.create('Doc', { owner: '1', org: 1, title: 'x' });
  else if (w === 'jobs') out = await ctx.jobs.run('anything', {}, 'k');
  return { out: JSON.stringify(out ?? null).slice(0, 800) };
}`,
    run: async (s) => {
      const out = [];
      for (const which of ['declared', 'rebound', 'undeclared', 'apply', 'create', 'jobs']) {
        const r = await s.call('u1', '/extension', { extension: 'Doc.peek', input: { which, n: 0 } });
        out.push(`${which}=${r.body.ok ? r.body.output.out : r.body.code}`);
      }
      return out.join(' | ');
    },
  },
  {
    id: 'C9-create-forbidden-vs-unique',
    claim: '생성 권한이 없는 호출자가 유일 제약에 걸리는 값을 넣어도 응답은 FORBIDDEN이어야 한다(권한 판정이 유일 제약보다 먼저). 공개 create와 확장 `ctx.data.create` 모두, 순차와 경합 모두.',
    def: `actor Member
resource Member { fields { id: Id; org: Int } rows read when true expose read { select id, org; sort id; budget { rows 20; depth 1; deadline 2s; cost 1000 } } }
resource Claim {
  fields { id: Id; owner: Member; tag: Text(1..9) }
  rows read when owner = actor
  unique tag
  expose read { select id, tag; sort id; budget { rows 20; depth 1; deadline 2s; cost 1000 } }
  expose create { allow owner = actor; fields owner, tag }
  extension write claimit { input { owner: Member.Id; tag: Text(1..9) } output { id: Text(0..200) } access create.Claim effect db deadline 3s implementation "x.claimit" }
  extension write claimcatch { input { owner: Member.Id; tag: Text(1..9) } output { id: Text(0..200) } access create.Claim effect db deadline 3s implementation "x.claimcatch" }
}`,
    seed: `INSERT INTO $S.member(id,org) VALUES (1,1),(2,1); INSERT INTO $S.claim(id,owner_id,tag) VALUES (1,1,'secret1'); SELECT setval(pg_get_serial_sequence('$S.claim','id'), 500)`,
    workers: `export async function claimit(input, ctx) { const id = await ctx.data.create('Claim', { owner: input.owner, tag: input.tag }); return { id: String(id) }; }
// 확장 작성자 코드가 ctx 오류를 잡아 호출자에게 그대로 돌려주는 최악의 경우도 본다(오류의 code·message가 원인을 가르는지).
export async function claimcatch(input, ctx) {
  try { const id = await ctx.data.create('Claim', { owner: input.owner, tag: input.tag }); return { id: String(id) }; }
  catch (e) { return { id: 'CAUGHT ' + (e.code ?? '') + ' ' + String(e.message ?? e).slice(0, 120) }; }
}`,
    run: async (s) => {
      const pub = (owner, tag) => bundle(s, 'u2', [{ create: 'Claim', values: { owner, tag } }]);
      const ext = (owner, tag, name = 'claimit') => apply(s, 'u2', { extension: `Claim.${name}`, input: { owner, tag } });
      const extShow = (r) => (r.body.ok ? r.body.output.id : r.body.code);
      const out = [];
      // u2는 Member 1의 claim을 만들 권한이 없다(allow owner = actor). 'secret1'은 u1의 숨은 claim 값이다.
      out.push(`public forbidden+existing=${code(await pub('1', 'secret1'))} forbidden+fresh=${code(await pub('1', 'fresh1'))}`);
      out.push(`extension forbidden+existing=${extShow(await ext('1', 'secret1'))} forbidden+fresh=${extShow(await ext('1', 'fresh2'))}`);
      out.push(`extension that catches the ctx error: forbidden+existing=${extShow(await ext('1', 'secret1', 'claimcatch'))} forbidden+fresh=${extShow(await ext('1', 'fresh5', 'claimcatch'))} allowed+existing=${extShow(await ext('2', 'secret1', 'claimcatch'))}`);
      // 권한이 있을 때(owner = actor)의 유일 제약 충돌은 K1과 같은 계열이다.
      out.push(`public allowed+existing=${code(await pub('2', 'secret1'))} allowed+fresh=${code(await pub('2', 'fresh3'))}`);
      out.push(`extension allowed+existing=${extShow(await ext('2', 'secret1'))} allowed+fresh=${extShow(await ext('2', 'fresh4'))}`);
      // 경합: 같은 값을 권한 없는 쪽과 있는 쪽이 동시에 보낸다. 권한 없는 쪽은 어떤 경우에도 FORBIDDEN이어야 한다.
      const seen = new Set();
      for (let round = 0; round < 8; round++) {
        const tag = `race${round}`;
        const results = await Promise.all([
          ...[0, 1, 2].map(() => ext('1', tag)),
          ...[0, 1, 2].map(() => ext('2', tag)),
        ]);
        results.slice(0, 3).forEach((r) => seen.add(extShow(r)));
      }
      out.push(`race forbidden-caller codes=${[...seen].sort().join(',')}`);
      return out.join(' | ');
    },
  },
  {
    id: 'D13-external-create-forbidden-vs-unique',
    claim: '외부 테이블 입양(`adopt-external`) 리소스의 create는 INSERT를 먼저 실행하고 allow를 나중에 평가한다. 권한 없는 호출자(member가 자신이 아님)가 유일 제약에 걸리는 값을 보내면 ALREADY_EXISTS, 걸리지 않으면 FORBIDDEN이라 숨은 행의 존재를 알아낸다.',
    external: {
      sourceSchema: 'ext_src',
      sql: `DROP SCHEMA IF EXISTS ext_src CASCADE; CREATE SCHEMA ext_src;
CREATE TABLE ext_src.members(member_id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY, org int NOT NULL DEFAULT 1);
CREATE TABLE ext_src.claims(claim_id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY, member_id bigint NOT NULL REFERENCES ext_src.members(member_id), tag text NOT NULL UNIQUE, status text NOT NULL DEFAULT 'open');
INSERT INTO ext_src.members(org) VALUES (1),(1);
INSERT INTO ext_src.claims(member_id,tag,status) VALUES (1,'secret1','open');`,
    },
    def: `actor Members
resource Members { fields { id: Id; org: Int default database } rows read when true
  expose read { select id, org; sort id; budget { rows 20; depth 1; deadline 2s; cost 1000 } } }
resource Claims {
  fields { id: Id; member: Members; tag: Text; status: Text default database }
  rows read when member = actor
  expose read { select id, tag, status; sort id; budget { rows 20; depth 1; deadline 2s; cost 1000 } }
  expose create { allow member = actor; fields member, tag }
}`,
    seed: '',
    quiet: true,
    run: async (s) => {
      const create = (member, tag) => bundle(s, 'u2', [{ create: 'Claims', values: { member, tag } }]);
      // u2는 Members 2다. Members 1의 claim을 만들 권한이 없다(`allow member = actor`). 'secret1'은 Members 1의 숨은 claim 값이다.
      const out = [`forbidden+existing=${code(await create('1', 'secret1'))}`, `forbidden+fresh=${code(await create('1', 'fresh1'))}`];
      out.push(`allowed+existing=${code(await create('2', 'secret1'))}`, `allowed+fresh=${code(await create('2', 'fresh2'))}`);
      return out.join(' ');
    },
  },
  {
    id: 'K4-external-create-allow-reads-default-field',
    claim: 'D13과 같지만 allow가 생략한 `default database` 필드(`status`)를 읽는다. 그 필드의 값은 INSERT 뒤에야 알 수 있어 INSERT 전 판정이 안 된다. 수정 뒤에도 남는 좁은 경우.',
    external: {
      sourceSchema: 'ext_src',
      sql: `DROP SCHEMA IF EXISTS ext_src CASCADE; CREATE SCHEMA ext_src;
CREATE TABLE ext_src.members(member_id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY, org int NOT NULL DEFAULT 1);
CREATE TABLE ext_src.claims(claim_id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY, member_id bigint NOT NULL REFERENCES ext_src.members(member_id), tag text NOT NULL UNIQUE, status text NOT NULL DEFAULT 'open');
INSERT INTO ext_src.members(org) VALUES (1),(1);
INSERT INTO ext_src.claims(member_id,tag,status) VALUES (1,'secret1','open');`,
    },
    def: `actor Members
resource Members { fields { id: Id; org: Int default database } rows read when true
  expose read { select id, org; sort id; budget { rows 20; depth 1; deadline 2s; cost 1000 } } }
resource Claims {
  fields { id: Id; member: Members; tag: Text; status: Text default database }
  rows read when member = actor
  expose read { select id, tag, status; sort id; budget { rows 20; depth 1; deadline 2s; cost 1000 } }
  expose create { allow member = actor and status = "open"; fields member, tag }
}`,
    seed: '',
    quiet: true,
    run: async (s) => {
      const create = (member, tag) => bundle(s, 'u2', [{ create: 'Claims', values: { member, tag } }]);
      return `forbidden+existing=${code(await create('1', 'secret1'))} forbidden+fresh=${code(await create('1', 'fresh1'))} allowed+existing=${code(await create('2', 'secret1'))} allowed+fresh=${code(await create('2', 'fresh2'))}`;
    },
  },
  // 알려진 한계(문서화된 존재 노출 계열). 고치지 않고 policy.md에 선택지만 적었다.
  {
    id: 'K1-unique-hidden-field',
    claim: '`unique code`에 가려진 필드가 있으면 create의 ALREADY_EXISTS로 값의 존재를 한 값씩 확인한다.',
    def: `actor Member
resource Member { fields { id: Id; role: Int } rows read when true expose read { select id, role; sort id; budget { rows 20; depth 1; deadline 2s; cost 1000 } } }
resource Doc {
  fields { id: Id; owner: Member; cap: Int; code: Int }
  rows read when true
  field code read when owner = actor
  unique code
  expose read { select id, cap, code; sort id; budget { rows 20; depth 1; deadline 2s; cost 1000 } }
  expose create { allow actor != null; fields owner, cap, code }
}`,
    seed: `INSERT INTO $S.member(id,role) VALUES (1,1),(2,1); INSERT INTO $S.doc(id,owner_id,cap,code) VALUES (1,1,0,4242); SELECT setval(pg_get_serial_sequence('$S.doc','id'), 500)`,
    run: async (s) => `code=1:${code(await bundle(s, 'u2', [{ create: 'Doc', values: { owner: '1', cap: 0, code: 1 } }]))} code=4242:${code(await bundle(s, 'u2', [{ create: 'Doc', values: { owner: '1', cap: 0, code: 4242 } }]))}`,
  },
  {
    id: 'K2-literal-increment-hidden-range',
    claim: '가려진 Int(0..100)에 리터럴 증가(`risk = risk + 1`)를 반복하면 범위 초과(BAD_VALUE)까지의 성공 횟수로 값이 드러난다.',
    def: `actor Member
resource Member { fields { id: Id; org: Int } rows read when true }
resource Doc {
  fields { id: Id; owner: Member; risk: Int(0..100); flag: Bool? }
  rows read when true
  field risk read when owner = actor
  transition bump { from true; allow actor != null; to risk = risk + 1 }
  expose apply bump { target id; bulk maxRows 1 }
  expose read { select id, flag, risk; sort id; budget { rows 20; depth 1; deadline 2s; cost 1000 } }
}`,
    seed: `INSERT INTO $S.member(id,org) VALUES (1,1),(2,1); INSERT INTO $S.doc(id,owner_id,risk,flag) VALUES (1,1,63,NULL)`,
    run: async (s) => {
      let ok = 0;
      for (let i = 0; i < 120; i++) {
        if (!(await apply(s, 'u2', { apply: 'Doc.bump', target: { ids: ['1'] } })).body.ok) break;
        ok++;
      }
      return `successes before BAD_VALUE=${ok} => hidden risk was ${100 - ok}`;
    },
  },
  {
    id: 'K3-fk-delete-hidden-child',
    claim: '숨은 자식 행이 참조하는 부모 삭제는 REFERENCE_CONFLICT, 자식 없는 부모는 성공해 숨은 행의 존재가 드러난다.',
    def: `actor Member
resource Member { fields { id: Id; org: Int } rows read when true }
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
  expose read { select id; sort id; budget { rows 20; depth 1; deadline 2s; cost 1000 } }
}`,
    seed: `INSERT INTO $S.member(id,org) VALUES (1,1),(2,1); INSERT INTO $S.parent(id,owner_id,name) VALUES (1,1,'kid-a'),(2,1,'plain'); INSERT INTO $S.child(id,parent_id,owner_id) VALUES (1,1,1)`,
    run: async (s) => `parent-with-hidden-child=${code(await apply(s, 'u2', { apply: 'Parent.drop', target: { ids: ['1'] } }))} plain-parent=${code(await apply(s, 'u2', { apply: 'Parent.drop', target: { ids: ['2'] } }))}`,
  },
];

const wanted = process.argv.slice(2);
let n = 0;
for (const sc of scenarios) {
  if (wanted.length && !wanted.some((w) => sc.id.startsWith(w))) continue;
  n++;
  let s;
  try {
    s = await setup(sc.id.toLowerCase().replace(/[^a-z0-9]/g, '_'), sc.def, sc.seed, { u1: 1, u2: 2, v1: 10 }, sc.workers ? { workers: true, mjs: sc.workers } : sc.external ? { external: sc.external } : {});
  } catch (e) {
    const m = String(e.message).match(/DEF\.[A-Z_]+/g) ?? [];
    console.log(`${sc.id.padEnd(34)} REJECTED ${[...new Set(m)].filter((c) => c !== 'DEF.INVALID_DEFINITION').join(',') || e.message.slice(0, 120)}`);
    continue;
  }
  try {
    console.log(`${sc.id.padEnd(34)} ${sc.id[0] === 'D' && !sc.quiet ? 'RECOVERED ' : ''}${await sc.run(s)}`);
  } finally {
    s.stop();
  }
}
console.log(`${n} scenarios`);

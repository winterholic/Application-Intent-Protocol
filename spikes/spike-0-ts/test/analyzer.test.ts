import assert from 'node:assert/strict';
import { test } from 'node:test';
import { compileSource } from '../src/compile.ts';
import { SHOP_SRC } from './helpers.ts';

const HEADER = `
actor User
enum Role { CUSTOMER ADMIN }
enum Status { OPEN CLOSED }
entity User { name: String role: Role }
entity Item { owner: User status: Status count: Int }
entity Box { owner: User parts: Part[] via box }
entity Part { box: Box item: Item qty: Int }
`;

function codes(body: string): string[] {
  return compileSource(HEADER + body, 't.aip').diagnostics.map((d) => d.code);
}

test('shop example compiles without diagnostics', () => {
  const { diagnostics } = compileSource(SHOP_SRC, 'app.aip');
  assert.deepEqual(diagnostics, []);
});

test('E301: operations without a policy are rejected', () => {
  assert.deepEqual(codes(`command Close(id: UUID) { load item: Item by id lock transaction { set item.status = CLOSED } }`), ['AIP-E301']);
  assert.deepEqual(codes(`query Items { from Item as i limit 10 select { id } }`), ['AIP-E301']);
});

test('E401: check-then-act on an unlocked row', () => {
  const src = `command Close(id: UUID) {
    load item: Item by id
    policy: owner(item.owner)
    require item.status == OPEN else NOT_OPEN
    transaction { set item.status = CLOSED }
  }`;
  assert.deepEqual(codes(src), ['AIP-E401']);
  assert.deepEqual(codes(src.replace('by id', 'by id lock')), []);
});

test('E213: no implicit loading through relations', () => {
  assert.deepEqual(codes(`command C(id: UUID) {
    load part: Part by id lock
    policy: owner(part.box.owner)
    transaction { set part.qty = 1 }
  }`), ['AIP-E213']);
});

test('E202: unknown enum value', () => {
  assert.deepEqual(codes(`command C(id: UUID) {
    load item: Item by id lock
    policy: owner(item.owner)
    transaction { set item.status = DELETED }
  }`), ['AIP-E202']);
});

test('E207: set through a reference inside each is ambiguous', () => {
  const src = `command C(id: UUID) {
    load box: Box by id lock
    policy: owner(box.owner)
    transaction { each box.parts as p { set p.item.count = 0 } }
  }`;
  assert.deepEqual(codes(src), ['AIP-E207']);
  assert.deepEqual(codes(src.replace('set p.item.count = 0', 'increment p.item.count by p.qty')), []);
});

test('E220: relations must be selected explicitly', () => {
  assert.deepEqual(codes(`query Q { from Box as b limit 5 policy: authenticated select { id parts } }`), ['AIP-E220']);
});

test('E105: id is implicit', () => {
  assert.deepEqual(compileSource('entity A { id: UUID }', 't.aip').diagnostics.map((d) => d.code), ['AIP-E105']);
});

test('E100: canonical clause order is enforced', () => {
  const d = compileSource(HEADER + `command C(id: UUID) { policy: authenticated load item: Item by id }`, 't.aip').diagnostics;
  assert.equal(d[0].code, 'AIP-E100');
  assert.match(d[0].message, /'load' must come before 'policy'/);
});

test('E303: owner() must point to the actor entity', () => {
  assert.deepEqual(codes(`command C(id: UUID) {
    load part: Part by id lock
    policy: owner(part.box)
    transaction { set part.qty = 1 }
  }`), ['AIP-E303']);
});

test('E304: unknown role', () => {
  assert.deepEqual(codes(`query Q { from Item as i limit 5 policy: role(ROOT) select { id } }`), ['AIP-E304']);
});

test('E205: create must set every required field', () => {
  assert.deepEqual(codes(`command C {
    idempotent
    policy: authenticated
    transaction { create Item { owner: actor, count: 0 } }
  }`), ['AIP-E205']);
});

test('W501: creating rows without idempotency is flagged', () => {
  assert.deepEqual(codes(`command C {
    policy: authenticated
    transaction { create Item { owner: actor, status: OPEN, count: 0 } }
  }`), ['AIP-W501']);
});

test('E216: one event, one shape', () => {
  assert.deepEqual(codes(`
    command A(id: UUID) { load item: Item by id lock policy: owner(item.owner) emit Touched { id: item.id } }
    command B(id: UUID) { load item: Item by id lock policy: owner(item.owner) emit Touched { itemId: item.id } }
  `), ['AIP-E216']);
});

test('W403: unbounded list query', () => {
  assert.deepEqual(codes(`query Q { from Item as i policy: authenticated select { id } }`), ['AIP-W403']);
});

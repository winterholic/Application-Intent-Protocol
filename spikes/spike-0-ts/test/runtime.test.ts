// Integration tests against a real PostgreSQL (database `aip_test`, recreated).
import assert from 'node:assert/strict';
import { after, before, test } from 'node:test';
import type pg from 'pg';
import { Runtime } from '../src/runtime/runtime.ts';
import { caller, compileOk, freshDb, seedShop, SHOP_SRC, type Seed } from './helpers.ts';

const shop = compileOk(SHOP_SRC, 'app.aip');
let pool: pg.Pool;
let rt: Runtime;
let s: Seed;
let c: ReturnType<typeof caller>;

before(async () => {
  pool = await freshDb(shop);
  s = await seedShop(pool);
  rt = new Runtime(shop, pool);
  c = caller(rt);
});

after(async () => {
  await pool.end();
});

const stock = async (id: string) => (await pool.query('SELECT stock FROM product WHERE id = $1', [id])).rows[0].stock as number;
const outbox = async () => (await pool.query('SELECT event, payload FROM _aip_outbox ORDER BY id')).rows;

test('order lifecycle: create → add items → pay → cancel restores stock', async () => {
  const order = await c.ok('command', 'CreateOrder', s.alice, {}, 'create-1');
  assert.equal(order.status, 'PENDING');
  assert.equal(order.user, s.alice);

  // Replaying the same idempotency key returns the stored result without a second insert.
  const replay = await c.ok('command', 'CreateOrder', s.alice, {}, 'create-1');
  assert.equal(replay.id, order.id);
  assert.equal((await pool.query('SELECT count(*)::int AS n FROM "order"')).rows[0].n, 1);

  // Two lines of the same product: the bulk restore below must add both back.
  await c.ok('command', 'AddItem', s.alice, { orderId: order.id, productId: s.productA, quantity: 2 }, 'add-1');
  await c.ok('command', 'AddItem', s.alice, { orderId: order.id, productId: s.productA, quantity: 1 }, 'add-2');
  const afterB = await c.ok('command', 'AddItem', s.alice, { orderId: order.id, productId: s.productB, quantity: 1 }, 'add-3');
  assert.equal(afterB.total, 350);
  assert.equal(await stock(s.productA), 2);
  assert.equal(await stock(s.productB), 2);

  const paid = await c.ok('command', 'PayOrder', s.alice, { orderId: order.id }, 'pay-1');
  assert.equal(paid.status, 'PAID');

  const late = await c.raw('command', 'AddItem', s.alice, { orderId: order.id, productId: s.productB, quantity: 1 }, 'add-4');
  assert.equal(late.ok, false);
  assert.deepEqual(!late.ok && [late.error.code, late.error.reason], ['AIP.PRECONDITION.FAILED', 'ORDER_NOT_EDITABLE']);

  const cancelled = await c.ok('command', 'CancelOrder', s.alice, { orderId: order.id });
  assert.equal(cancelled.status, 'CANCELLED');
  assert.equal(await stock(s.productA), 5);
  assert.equal(await stock(s.productB), 3);

  const again = await c.raw('command', 'CancelOrder', s.alice, { orderId: order.id });
  assert.deepEqual(!again.ok && again.error.reason, 'ORDER_NOT_CANCELLABLE');

  assert.deepEqual((await outbox()).map((e) => e.event), ['OrderCreated', 'OrderPaid', 'OrderCancelled']);
  assert.deepEqual((await outbox())[1].payload, { orderId: order.id, amount: 350 });
});

test('authorization: owner, role, anonymous', async () => {
  const order = await c.ok('command', 'CreateOrder', s.alice, {}, 'create-auth');

  const bob = await c.raw('command', 'AddItem', s.bob, { orderId: order.id, productId: s.productA, quantity: 1 }, 'bob-1');
  assert.deepEqual(!bob.ok && [bob.status, bob.error.code], [403, 'AIP.AUTH.FORBIDDEN']);

  const anon = await c.raw('command', 'CreateOrder', null, {}, 'anon-1');
  assert.deepEqual(!anon.ok && [anon.status, anon.error.code], [401, 'AIP.AUTH.UNAUTHENTICATED']);

  const forged = await c.raw('command', 'CreateOrder', '00000000-0000-0000-0000-000000000000', {}, 'forged-1');
  assert.deepEqual(!forged.ok && forged.error.code, 'AIP.AUTH.UNAUTHENTICATED');

  const detailBob = await c.raw('query', 'OrderDetail', s.bob, { orderId: order.id });
  assert.deepEqual(!detailBob.ok && detailBob.error.code, 'AIP.AUTH.FORBIDDEN');
  const detailAdmin = await c.ok('query', 'OrderDetail', s.admin, { orderId: order.id });
  assert.equal(detailAdmin.user.name, 'alice');

  const missing = await c.raw('query', 'OrderDetail', s.admin, { orderId: '00000000-0000-0000-0000-000000000001' });
  assert.deepEqual(!missing.ok && missing.error.code, 'AIP.NOT_FOUND');

  const byStatus = await c.raw('query', 'OrdersByStatus', s.alice, { status: 'PENDING' });
  assert.deepEqual(!byStatus.ok && byStatus.error.code, 'AIP.AUTH.FORBIDDEN');
  assert.ok((await c.ok('query', 'OrdersByStatus', s.admin, { status: 'PENDING' })).length >= 1);

  // Bob's list never contains Alice's orders: the policy is part of the SQL filter.
  assert.deepEqual(await c.ok('query', 'MyOrders', s.bob), []);
});

test('input validation and preconditions', async () => {
  const order = await c.ok('command', 'CreateOrder', s.alice, {}, 'create-v');
  const extra = await c.raw('command', 'AddItem', s.alice, { orderId: order.id, productId: s.productA, quantity: 1, price: 0 }, 'v-1');
  assert.deepEqual(!extra.ok && [extra.error.code, extra.error.path], ['AIP.INPUT.INVALID', 'price']);

  const zero = await c.raw('command', 'AddItem', s.alice, { orderId: order.id, productId: s.productA, quantity: 0 }, 'v-2');
  assert.deepEqual(!zero.ok && zero.error.reason, 'INVALID_QUANTITY');

  const tooMany = await c.raw('command', 'AddItem', s.alice, { orderId: order.id, productId: s.productB, quantity: 99 }, 'v-3');
  assert.deepEqual(!tooMany.ok && tooMany.error.reason, 'OUT_OF_STOCK');

  const noKey = await c.raw('command', 'AddItem', s.alice, { orderId: order.id, productId: s.productA, quantity: 1 });
  assert.deepEqual(!noKey.ok && noKey.error.code, 'AIP.INPUT.IDEMPOTENCY_KEY_REQUIRED');

  await c.ok('command', 'AddItem', s.alice, { orderId: order.id, productId: s.productA, quantity: 1 }, 'v-4');
  const reused = await c.raw('command', 'AddItem', s.alice, { orderId: order.id, productId: s.productA, quantity: 2 }, 'v-4');
  assert.deepEqual(!reused.ok && reused.error.code, 'AIP.IDEMPOTENCY.KEY_REUSED');
  await c.ok('command', 'CancelOrder', s.alice, { orderId: order.id });
});

test('concurrency: parallel AddItem never oversells', async () => {
  const pid = (await pool.query(`INSERT INTO product (name, price, stock) VALUES ('Limited', 10, 5) RETURNING id`)).rows[0].id;
  const orders = [];
  for (let i = 0; i < 10; i++) orders.push(await c.ok('command', 'CreateOrder', s.alice, {}, `race-order-${i}`));
  const results = await Promise.all(orders.map((o, i) => c.raw('command', 'AddItem', s.alice, { orderId: o.id, productId: pid, quantity: 1 }, `race-${i}`)));
  assert.equal(results.filter((r) => r.ok).length, 5);
  assert.deepEqual([...new Set(results.filter((r) => !r.ok).map((r) => !r.ok && r.error.reason))], ['OUT_OF_STOCK']);
  assert.equal(await stock(pid), 0);
});

test('concurrency: the same idempotency key in parallel executes once', async () => {
  const before = (await pool.query('SELECT count(*)::int AS n FROM "order"')).rows[0].n;
  const results = await Promise.all(Array.from({ length: 8 }, () => c.raw('command', 'CreateOrder', s.bob, {}, 'dup-parallel')));
  assert.ok(results.every((r) => r.ok));
  assert.equal(new Set(results.map((r) => r.ok && (r.data as { id: string }).id)).size, 1);
  assert.equal((await pool.query('SELECT count(*)::int AS n FROM "order"')).rows[0].n, before + 1);
});

test('query plan: statement count is fixed regardless of row count (no N+1)', async () => {
  for (let i = 0; i < 20; i++) {
    const o = await c.ok('command', 'CreateOrder', s.bob, {}, `n1-${i}`);
    await c.ok('command', 'AddItem', s.bob, { orderId: o.id, productId: i % 2 ? s.productA : s.productB, quantity: 1 }, `n1-item-${i}`);
    await c.ok('command', 'CancelOrder', s.bob, { orderId: o.id });
  }
  const r = await c.raw('query', 'MyOrders', s.bob);
  assert.ok(r.ok);
  const data = r.data as { total: number; items: { quantity: number; product: { name: string } }[] }[];
  assert.ok(data.length >= 20);
  assert.ok(data.every((o) => o.items.every((it) => ['Keyboard', 'Mouse'].includes(it.product.name))));
  // root + items + items.product; policy/actor lookup is not part of the plan
  const selects = r.trace.filter((sql) => sql.startsWith('SELECT'));
  assert.equal(selects.length, 3);
});

test('invariants are enforced by the database even when a require is missing', async () => {
  const loose = compileOk(SHOP_SRC.replace('  require product.stock >= quantity else OUT_OF_STOCK\n', ''), 'loose.aip');
  const lrt = caller(new Runtime(loose, pool));
  const o = await lrt.ok('command', 'CreateOrder', s.alice, {}, 'inv-order');
  const r = await lrt.raw('command', 'AddItem', s.alice, { orderId: o.id, productId: s.productB, quantity: 999 }, 'inv-1');
  assert.deepEqual(!r.ok && [r.error.code, r.error.reason, r.error.path], ['AIP.INVARIANT.VIOLATED', 'stock_non_negative', 'Product']);
  assert.equal(await stock(s.productB), 3);
});

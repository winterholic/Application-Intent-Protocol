// End-to-end: generated TypeScript client → HTTP runtime → PostgreSQL.
import assert from 'node:assert/strict';
import { mkdirSync, writeFileSync } from 'node:fs';
import type { AddressInfo } from 'node:net';
import { after, before, test } from 'node:test';
import type pg from 'pg';
import { generateClient } from '../src/clientgen.ts';
import { describe } from '../src/contract.ts';
import { Runtime } from '../src/runtime/runtime.ts';
import { createAipServer } from '../src/runtime/server.ts';
import { compileOk, freshDb, seedShop, SHOP_SRC, type Seed } from './helpers.ts';

const shop = compileOk(SHOP_SRC, 'app.aip');
let pool: pg.Pool;
let server: ReturnType<typeof createAipServer>;
let baseUrl: string;
let s: Seed;

before(async () => {
  pool = await freshDb(shop);
  s = await seedShop(pool);
  server = createAipServer(new Runtime(shop, pool));
  await new Promise<void>((r) => server.listen(0, r));
  baseUrl = `http://localhost:${(server.address() as AddressInfo).port}`;
});

after(async () => {
  server.close();
  await pool.end();
});

test('describe exposes intents, effects and error reasons', async () => {
  const contract = await (await fetch(`${baseUrl}/aip/describe`)).json();
  const cancel = contract.commands.CancelOrder;
  assert.deepEqual(cancel.input, { orderId: 'UUID' });
  assert.equal(cancel.policy, 'owner(order.user) | role(ADMIN)');
  assert.deepEqual(cancel.effects.writes, ['Product.stock', 'Order.status']);
  assert.deepEqual(cancel.effects.emits, ['OrderCancelled']);
  assert.deepEqual(cancel.errors.find((e: { code: string }) => e.code === 'AIP.PRECONDITION.FAILED').reasons, ['ORDER_NOT_CANCELLABLE']);
  // Entities are never exposed as a generic data API; only declared intents are.
  assert.deepEqual(Object.keys(contract).sort(), ['aip', 'commands', 'enums', 'events', 'queries', 'transport']);
});

test('generated client drives the full flow over HTTP', async () => {
  const dir = new URL('./.generated/', import.meta.url);
  mkdirSync(dir, { recursive: true });
  const file = new URL('client.ts', dir);
  writeFileSync(file, generateClient(describe(shop)));
  const { createClient, AipClientError } = await import(file.href);

  const alice = createClient({ baseUrl, actor: s.alice });
  const order = await alice.commands.CreateOrder({}, { idempotencyKey: 'http-1' });
  await alice.commands.AddItem({ orderId: order.id, productId: s.productA, quantity: 2 }, { idempotencyKey: 'http-2' });
  const mine = await alice.queries.MyOrders({});
  assert.equal(mine[0].items[0].product.name, 'Keyboard');
  assert.equal(mine[0].total, 200);

  const bob = createClient({ baseUrl, actor: s.bob });
  await assert.rejects(bob.commands.CancelOrder({ orderId: order.id }), (e: InstanceType<typeof AipClientError>) => {
    assert.equal(e.status, 403);
    assert.equal(e.body.code, 'AIP.AUTH.FORBIDDEN');
    return true;
  });
});

test('unknown operations and malformed bodies get structured errors', async () => {
  const r1 = await fetch(`${baseUrl}/aip/command/DropAllTables`, { method: 'POST', body: '{}' });
  assert.equal(r1.status, 404);
  assert.equal((await r1.json()).error.code, 'AIP.REQUEST.UNKNOWN_OPERATION');
  const r2 = await fetch(`${baseUrl}/aip/query/MyOrders`, { method: 'POST', body: '{nope', headers: { 'x-aip-actor': s.alice } });
  assert.equal(r2.status, 400);
  assert.equal((await r2.json()).error.code, 'AIP.REQUEST.MALFORMED');
});

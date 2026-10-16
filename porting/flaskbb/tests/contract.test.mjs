import assert from 'node:assert/strict';
import {test} from 'node:test';
import {spawnSync} from 'node:child_process';
import {readFileSync} from 'node:fs';
import {resolve} from 'node:path';
import {fileURLToPath} from 'node:url';

const track = resolve(fileURLToPath(new URL('..', import.meta.url)));
const root = resolve(track, '../..');
const definition = resolve(track, 'app.aip');
const binary = resolve(root, 'target/debug/aip');

test('FlaskBB port contract parses and retains the full source workflow denominator', () => {
  const checked = spawnSync(binary, ['service', 'check', definition], {encoding: 'utf8'});
  assert.equal(checked.status, 0, `${checked.stdout}\n${checked.stderr}`);
  const inventory = readFileSync(resolve(track, 'INVENTORY.md'), 'utf8');
  for (const id of Array.from({length: 17}, (_, i) => `F${String(i + 1).padStart(2, '0')}`)) {
    assert.match(inventory, new RegExp(`\\| ${id} \\|`), `missing source workflow ${id}`);
  }
  assert.match(inventory, /분모 v0\.1 검토본/);
});

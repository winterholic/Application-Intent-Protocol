import assert from 'node:assert/strict';
import {test} from 'node:test';
import {spawnSync} from 'node:child_process';
const harness = new URL('./harness.mjs', import.meta.url).href;
const valid='host=127.0.0.1 port=55497 dbname=aip_test_flaskbb_guard user=fixture';
function probe(extra={}) {
  const env=Object.fromEntries(Object.entries(process.env).filter(([key])=>!key.startsWith('PG')&&!['AIP_TEST_DATABASE_URL','AIP_FLASKBB_TEST_DATABASE_URL'].includes(key)));
  return spawnSync(process.execPath,['--input-type=module','-e',`import {dbUrl} from ${JSON.stringify(harness)}; console.log(dbUrl);`],{env:{...env,...extra},encoding:'utf8',timeout:10000});
}
test('FlaskBB fixture refuses missing DSN before setup or network access',()=>{
  const r=probe();assert.notEqual(r.status,0);assert.match(r.stderr,/AIP_TEST_DATABASE_URL is required/);
});
test('FlaskBB fixture shares the strict main DB guard without fallback or alias drift',()=>{
  for (const [extra,reason] of [
    [{AIP_TEST_DATABASE_URL:'host=127.0.0.1 port=5432 dbname=aip_test_fixture user=fixture'},/non-default/],
    [{AIP_TEST_DATABASE_URL:'host=127.0.0.1 port=55497 dbname=postgres user=fixture'},/aip_test_/],
    [{AIP_TEST_DATABASE_URL:valid,PGSERVICE:'legacy'},/PGSERVICE must be unset/],
    [{AIP_FLASKBB_TEST_DATABASE_URL:valid},/AIP_TEST_DATABASE_URL is required/],
    [{AIP_TEST_DATABASE_URL:valid,AIP_FLASKBB_TEST_DATABASE_URL:valid.replace('55497','55498')},/must match/],
  ]) {const r=probe(extra);assert.notEqual(r.status,0);assert.match(r.stderr,reason);}
  for (const extra of [{AIP_TEST_DATABASE_URL:valid},{AIP_TEST_DATABASE_URL:valid,AIP_FLASKBB_TEST_DATABASE_URL:valid}]) {
    const r=probe(extra);assert.equal(r.status,0,r.stderr);assert.equal(r.stdout.trim(),valid);
  }
});

test('all standalone FlaskBB DB suites refuse missing DSN before fixture setup',()=>{
  const env=Object.fromEntries(Object.entries(process.env).filter(([key])=>!key.startsWith('PG')&&!['AIP_TEST_DATABASE_URL','AIP_FLASKBB_TEST_DATABASE_URL','NODE_TEST_CONTEXT','NODE_CHANNEL_FD'].includes(key)));
  for (const name of ['aip-read.test.mjs','features.test.mjs','provisioning.test.mjs','mail-jobs.test.mjs']) {
    const module=new URL(name,import.meta.url).href;
    const r=spawnSync(process.execPath,['--input-type=module','-e',`await import(${JSON.stringify(module)});`],{env,encoding:'utf8',timeout:10000});
    assert.notEqual(r.status,0,name);assert.match(r.stdout+r.stderr,/AIP_TEST_DATABASE_URL is required/,name);
  }
});

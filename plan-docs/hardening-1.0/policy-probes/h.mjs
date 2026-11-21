import { generateKeyPairSync, createSign } from 'node:crypto';
import { spawn, spawnSync } from 'node:child_process';
import { mkdtempSync, writeFileSync, mkdirSync } from 'node:fs';
import { join } from 'node:path';
const AIP = process.env.AIP_BIN ?? 'target/debug/aip';
const DSN = process.env.AIP_TEST_DATABASE_URL ?? 'host=127.0.0.1 port=15433 dbname=aip_test_ci user=runner';
// 컨테이너 이름을 주면 `docker exec`로, 안 주면 호스트의 `psql`로 DSN에 붙는다.
const PG_CONTAINER = process.env.AIP_PROBE_PG_CONTAINER;
const b64 = (b) => Buffer.from(b).toString('base64url');
const env = { ...process.env, AIP_TEST_DATABASE: DSN };
export function psql(sql) {
  const flags = ['-v', 'ON_ERROR_STOP=1', '-q', '-t', '-A'];
  const r = PG_CONTAINER
    ? spawnSync('docker', ['exec', '-i', PG_CONTAINER, 'psql', '-U', 'runner', '-d', 'aip_test_ci', ...flags], { input: sql, encoding: 'utf8' })
    : spawnSync('psql', [DSN, ...flags], { input: sql, encoding: 'utf8' });
  if (r.status !== 0) throw new Error('psql: ' + r.stderr);
  return r.stdout;
}
function run(args) {
  const r = spawnSync(AIP, ['service', ...args], { encoding: 'utf8', env });
  if (r.status !== 0) throw new Error(args.join(' ') + ': ' + r.stderr + r.stdout);
  return r.stdout;
}
export async function setup(name, src, seedSql, subjects, opts = {}) {
  const dir = mkdtempSync('/tmp/aip-h-' + name + '-');
  const schema = 'aip_h_' + name.replace(/[^a-z0-9]/g, '_');
  psql(`DROP SCHEMA IF EXISTS ${schema} CASCADE`);
  const { privateKey, publicKey } = generateKeyPairSync('rsa', { modulusLength: 2048 });
  const jwk = publicKey.export({ format: 'jwk' });
  writeFileSync(join(dir, 'jwks.json'), JSON.stringify({ keys: [{ kty: 'RSA', kid: 'current', alg: 'RS256', use: 'sig', n: jwk.n, e: jwk.e }] }));
  const cfg = join(dir, 'service.json');
  const conf = { schema, database_url_env: 'AIP_TEST_DATABASE', listen: '127.0.0.1:0', wire: 'decimal', allowed_origins: ['https://app.example.com'], auth: { issuer: 'https://issuer.example', audience: 'aip-api', jwks: { kind: 'file', path: 'jwks.json' } } };
  if (opts.workers) {
    mkdirSync(join(dir, 'extensions'));
    writeFileSync(join(dir, 'extensions', 'x.mjs'), opts.mjs);
    conf.workers = { directory: join(dir, 'extensions'), language: 'node', enable_writes: true, isolation: process.platform === 'darwin' ? 'mac_net_deny' : 'auto' };
  }
  const file = join(dir, 'app.aip');
  writeFileSync(file, src);
  if (opts.external) {
    // 기존 테이블을 view로 입양하는 경로(`adopt-external`). 원본 스키마는 호출자가 `externalSql`로 만든다.
    psql(opts.external.sql);
    writeFileSync(cfg, JSON.stringify({ ...conf, migrate_database_url_env: 'AIP_TEST_DATABASE' }));
    run(['introspect', '--config', cfg, '--source-schema', opts.external.sourceSchema, '--out', join(dir, 'draft.aip'), '--mapping-out', join(dir, 'mapping.json')]);
    writeFileSync(cfg, JSON.stringify({ ...conf, migrate_database_url_env: 'AIP_TEST_DATABASE', external_mapping: join(dir, 'mapping.json') }));
    const plan = JSON.parse(run(['adopt-external', file, '--config', cfg]));
    run(['adopt-external', file, '--config', cfg, '--apply', plan.digest]);
    run(['provision', '--config', cfg]);
  } else {
    writeFileSync(cfg, JSON.stringify(conf));
    run(['init', file, '--config', cfg]);
    psql(seedSql.replaceAll('$S', schema));
  }
  for (const [sub, actor] of Object.entries(subjects)) {
    try { run(['principal', '--config', cfg, 'bind', '--subject', sub, '--actor', String(actor)]); } catch { /* 그 actor 행이 없는 시나리오는 건너뛴다 */ }
  }
  const child = spawn(AIP, ['service', 'serve', file, '--config', cfg], { env, stdio: ['ignore', 'pipe', 'pipe'] });
  const line = await new Promise((res, rej) => {
    let buf = ''; let err = '';
    child.stderr.on('data', (d) => (err += d));
    child.stdout.on('data', (d) => { buf += d; if (buf.includes('\n')) res(buf.split('\n')[0]); });
    child.on('exit', () => rej(new Error('serve exited: ' + err)));
    setTimeout(() => rej(new Error('serve timeout ' + err)), 15000);
  });
  const L = JSON.parse(line);
  const token = (sub) => {
    const now = Math.floor(Date.now() / 1000);
    const h = b64(JSON.stringify({ alg: 'RS256', typ: 'at+jwt', kid: 'current' }));
    const p = b64(JSON.stringify({ iss: 'https://issuer.example', aud: 'aip-api', sub, iat: now, nbf: now, exp: now + 600 }));
    const s = createSign('RSA-SHA256'); s.update(h + '.' + p);
    return h + '.' + p + '.' + s.sign(privateKey).toString('base64url');
  };
  const call = async (sub, path, body, headers = {}) => {
    const r = await fetch('http://' + L.address + path, { method: 'POST', headers: { authorization: 'Bearer ' + (sub === null ? 'x' : token(sub)), 'x-aip-contract': L.fingerprint, origin: 'https://app.example.com', 'content-type': 'application/json', ...headers }, body: JSON.stringify(body) });
    const t = await r.text();
    let j; try { j = JSON.parse(t); } catch { j = t; }
    if (j && typeof j === 'object') delete j.requestId;
    return { status: r.status, body: j };
  };
  const stop = () => {
    child.kill('SIGTERM');
    psql(`DROP SCHEMA IF EXISTS ${schema} CASCADE`);
    if (opts.external) psql(`DROP SCHEMA IF EXISTS ${opts.external.sourceSchema} CASCADE`);
  };
  return { dir, schema, call, stop, address: L.address };
}

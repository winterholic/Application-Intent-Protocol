// Shared real-PostgreSQL fixture for the FlaskBB port tests: dynamic schema,
// `aip service init/serve` on loopback, ephemeral RS256 issuer, official SDK.
// The database must be explicitly supplied through the main isolated-test DSN guard.
import {createHash, generateKeyPairSync, sign, randomUUID} from 'node:crypto';
import {requireTestDatabase} from '../../../product/test-support/postgres-guard.mjs';
import {stopOwnedProcess} from './mail-fixture.mjs';
import {mkdtempSync, readFileSync, rmSync, writeFileSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join, resolve} from 'node:path';
import {once} from 'node:events';
import {createInterface} from 'node:readline';
import {pathToFileURL} from 'node:url';
import {spawn, spawnSync} from 'node:child_process';

const track = resolve(new URL('..', import.meta.url).pathname);
export const root = resolve(track, '../..');
export const binary = process.env.AIP_BINARY || join(root, 'target/debug/aip');
export const definition = join(track, 'app.aip');
export const dbUrl = requireTestDatabase().raw;
if (process.env.AIP_FLASKBB_TEST_DATABASE_URL && process.env.AIP_FLASKBB_TEST_DATABASE_URL !== dbUrl) {
  throw new Error('AIP_FLASKBB_TEST_DATABASE_URL must match AIP_TEST_DATABASE_URL');
}
const b64 = value => Buffer.from(JSON.stringify(value)).toString('base64url');

export const GROUPS_SQL = schema => `INSERT INTO "${schema}"."group" (id,name,guest,admin,super_mod,mod,banned,editpost,deletepost,deletetopic,posttopic,postreply,viewhidden,makehidden,mod_edituser,mod_banuser,description) VALUES
  (1,'Guest',true,false,false,false,false,false,false,false,false,false,false,false,false,false,NULL),
  (2,'Member',false,false,false,false,false,true,false,false,true,true,false,false,false,false,NULL),
  (3,'Administrator',false,true,false,false,false,true,true,true,true,true,true,true,true,true,NULL),
  (4,'Moderator',false,false,false,true,false,true,true,true,true,true,false,false,true,false,NULL),
  (5,'Super Moderator',false,false,true,false,false,true,true,true,true,true,true,true,true,false,NULL);`;

export const USERS_SQL = schema => `INSERT INTO "${schema}".user (id,username,email,date_joined,lastseen,primary_group_id,activated,language) VALUES
  (1,'guest-fixture','guest@example.test',now(),now(),1,true,'en'),
  (2,'member-alice','alice@example.test',now(),now(),2,true,'en'),
  (3,'admin-bob','bob@example.test',now(),now(),3,true,'en'),
  (4,'mod-editor','editor@example.test',now(),now(),4,true,'en'),
  (5,'super-mod','supermod@example.test',now(),now(),5,true,'en'),
  (6,'member-carol','carol@example.test',now(),now(),2,true,'en');`;

/**
 * Start a fresh schema and listener. `seed(sql, q, schema, run)` inserts rows after init;
 * `actors` maps token subject -> actor id and is bound as service principals.
 * Returns helpers and `stop()`; always call stop() in a finally block.
 */
export async function startFixture({prefix = 'aip-flaskbb-test-', seed, actors = {}, serviceExtra = {}} = {}) {
  const temp = mkdtempSync(join(tmpdir(), prefix));
  const schema = `aip_flaskbb_${randomUUID().replaceAll('-', '').slice(0, 10)}`;
  const env = {...process.env, AIP_FLASKBB_DATABASE: dbUrl};
  const run = (command, args, options = {}) => {
    const result = spawnSync(command, args, {cwd: root, env, encoding: 'utf8', timeout: 30000, maxBuffer: 4 << 20, ...options});
    if (result.status !== 0) throw new Error(`${command} ${args.join(' ')}\n${result.stdout}\n${result.stderr}`);
    return result.stdout.trim();
  };
  const sql = statement => run('psql', ['-X', '-v', 'ON_ERROR_STOP=1', '-At', '-d', dbUrl, '-c', statement]);
  const config = join(temp, 'service.json');
  const jwksPath = join(temp, 'jwks.json');
  const bindingPath = join(temp, 'binding.ts');
  const sdkRoot = join(temp, 'sdk');
  let server;
  const stop = async () => {
    await stopOwnedProcess(server);
    const infra = `aip_infra_${createHash('sha256').update(schema).digest('hex').slice(0, 52)}`;
    try { sql(`DROP SCHEMA IF EXISTS "${schema}" CASCADE; DROP SCHEMA IF EXISTS "${infra}" CASCADE;`); }
    finally { rmSync(temp, {recursive: true, force: true}); }
  };
  try {
    const {publicKey, privateKey} = generateKeyPairSync('rsa', {modulusLength: 2048});
    const jwk = {...publicKey.export({format: 'jwk'}), kid: 'flaskbb-fixture', alg: 'RS256', use: 'sig', key_ops: ['verify']};
    writeFileSync(jwksPath, JSON.stringify({keys: [jwk]}));
    writeFileSync(config, JSON.stringify({
      schema,
      database_url_env: 'AIP_FLASKBB_DATABASE',
      migrate_database_url_env: 'AIP_FLASKBB_DATABASE',
      listen: '127.0.0.1:0',
      wire: 'decimal',
      allowed_origins: ['http://127.0.0.1:58080'],
      auth: {issuer: 'https://flaskbb.test', audience: 'aip-flaskbb', jwks: {kind: 'file', path: jwksPath}},
      ...serviceExtra,
    }));
    const init = JSON.parse(run(binary, ['service', 'init', definition, '--config', config]));
    if (!init.ok) throw new Error(`service init: ${JSON.stringify(init)}`);
    const q = `"${schema}"`;
    if (seed) seed(sql, q, schema, run);
    for (const [subject, actor] of Object.entries(actors)) {
      run(binary, ['service', 'principal', '--config', config, 'bind', '--subject', subject, '--actor', String(actor)]);
    }
    run(process.execPath, ['product/tools/build-sdk.mjs', '--out', sdkRoot]);
    run(binary, ['service', 'gen', definition, '--out', bindingPath]);
    const bindingSource = readFileSync(bindingPath, 'utf8');
    const section = name => JSON.parse(bindingSource.match(new RegExp(`export const ${name} = (.*) as const;`))[1]);
    const contract = {
      fingerprint: bindingSource.match(/export const contractFingerprint = "([0-9a-f]+)"/)[1],
      idWire: 'decimal-string-v13',
      identityKinds: JSON.parse(bindingSource.match(/identityKinds: (.*), fingerprint:/)[1]),
      extensions: section('extensionDescriptors'),
      readDescriptors: section('readDescriptors'),
      operations: section('operationDescriptors'),
    };
    const killServer = async () => {
      await stopOwnedProcess(server);
      server = undefined;
    };
    const start = async file => {
      await killServer();
      server = spawn(binary, ['service', 'serve', file, '--config', config], {cwd: temp, env, stdio: ['ignore', 'pipe', 'pipe']});
      server.unref();
      let stderr = '';
      server.stderr.on('data', data => {stderr += data;});
      const lines = createInterface({input: server.stdout});
      const started = await Promise.race([
        once(lines, 'line').then(([line]) => JSON.parse(line)),
        once(server, 'exit').then(([code]) => {throw new Error(`serve exit ${code}: ${stderr}`);}),
        new Promise((_, reject) => setTimeout(() => reject(new Error(`serve timeout: ${stderr}`)), 20000).unref()),
      ]);
      if (!started.ok) throw new Error(`serve: ${JSON.stringify(started)}`);
      lines.close();
      server.stdout.destroy();
      server.stderr.destroy();
      return started;
    };
    let started = await start(definition);
    const {connect} = await import(pathToFileURL(join(sdkRoot, 'product/sdk/index.js')).href);
    const token = subject => {
      const now = Math.floor(Date.now() / 1000);
      const unsigned = `${b64({alg: 'RS256', typ: 'at+jwt', kid: jwk.kid})}.${b64({iss: 'https://flaskbb.test', aud: 'aip-flaskbb', sub: subject, iat: now, nbf: now, exp: now + 600})}`;
      return `${unsigned}.${sign('RSA-SHA256', Buffer.from(unsigned), privateKey).toString('base64url')}`;
    };
    const client = subject => connect(`http://${started.address}`, token(subject), contract);
    // Restart serve against another definition file (after a migration); returns the new address.
    const restart = async file => {
      started = await start(file);
      return started.address;
    };
    return {schema, q, sql, run, client, token, contract, config, temp, stop, restart, killServer, get address() {return started.address;}};
  } catch (error) {
    await stop();
    throw error;
  }
}

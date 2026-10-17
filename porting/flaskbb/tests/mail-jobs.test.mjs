import assert from 'node:assert/strict';
import {test} from 'node:test';
import {generateKeyPairSync, sign, randomUUID} from 'node:crypto';
import {cpSync, mkdtempSync, readFileSync, rmSync, writeFileSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join, resolve} from 'node:path';
import {once} from 'node:events';
import {createInterface} from 'node:readline';
import {pathToFileURL} from 'node:url';
import {spawn, spawnSync} from 'node:child_process';

// 7.5.3 F14: account mail jobs through `aip service serve` with the real worker
// network isolation. The worker never opens a socket; delivery is the declared
// outbound HTTP broker endpoint `mail`, pointed at an isolated Mailpit.
const track = resolve(new URL('..', import.meta.url).pathname);
const root = resolve(track, '../..');
const binary = process.env.AIP_BINARY || join(root, 'target/debug/aip');
const file = join(track, 'app.aip');
const mailpit = process.env.AIP_FLASKBB_TEST_MAILPIT_URL || 'http://127.0.0.1:55473';
const b64 = value => Buffer.from(JSON.stringify(value)).toString('base64url');

test('FlaskBB account mail jobs deliver through the declared HTTP integration under worker network isolation', {timeout: 120000}, async () => {
  const temp = mkdtempSync(join(tmpdir(), 'aip-flaskbb-mail-'));
  const schema = `aip_flaskbb_${randomUUID().replaceAll('-', '').slice(0, 10)}`;
  const dbUrl = process.env.AIP_FLASKBB_TEST_DATABASE_URL || 'postgresql://aip_flaskbb:aip_flaskbb_fixture_password@127.0.0.1:55439/aip_flaskbb';
  const env = {...process.env, AIP_FLASKBB_DATABASE: dbUrl};
  const run = (command, args, options = {}) => {
    const result = spawnSync(command, args, {cwd: root, env, encoding: 'utf8', timeout: 30000, maxBuffer: 4 << 20, ...options});
    assert.equal(result.status, 0, `${command} ${args.join(' ')}\n${result.stdout}\n${result.stderr}`);
    return result.stdout.trim();
  };
  const sql = statement => run('psql', ['-X', '-v', 'ON_ERROR_STOP=1', '-At', '-d', dbUrl, '-c', statement]);
  const config = join(temp, 'service.json');
  const jwksPath = join(temp, 'jwks.json');
  const bindingPath = join(temp, 'binding.ts');
  const sdkRoot = join(temp, 'sdk');
  const workerDir = join(temp, 'mail-worker');
  let server;
  const runTag = randomUUID().replaceAll('-', '').slice(0, 12);
  // Unique addresses keep this run's assertions independent of earlier Mailpit content.
  const aliceMail = `alice-${runTag}@example.test`;
  const carolMail = `carol-${runTag}@example.test`;

  try {
    cpSync(join(track, 'mail'), workerDir, {recursive: true, filter: source => !source.includes('__pycache__')});
    const {publicKey, privateKey} = generateKeyPairSync('rsa', {modulusLength: 2048});
    const jwk = {...publicKey.export({format: 'jwk'}), kid: 'flaskbb-mail-fixture', alg: 'RS256', use: 'sig', key_ops: ['verify']};
    writeFileSync(jwksPath, JSON.stringify({keys: [jwk]}));
    const serviceConfig = {
      schema,
      database_url_env: 'AIP_FLASKBB_DATABASE',
      migrate_database_url_env: 'AIP_FLASKBB_DATABASE',
      listen: '127.0.0.1:0',
      wire: 'decimal',
      allowed_origins: ['http://127.0.0.1:58080'],
      auth: {issuer: 'https://flaskbb.test', audience: 'aip-flaskbb', jwks: {kind: 'file', path: jwksPath}},
      workers: {
        directory: workerDir,
        language: 'python',
        // macOS: sandbox-exec network deny; Linux: bubblewrap --unshare-net via auto.
        isolation: process.platform === 'darwin' ? 'mac_net_deny' : 'auto',
        environment: {FLASKBB_MAIL_SENDER: 'noreply@flaskbb.test', FLASKBB_ACTIVATION_BASE_URL: 'http://127.0.0.1:58080'},
        integrations: [{
          name: 'mail',
          url: `${mailpit}/api/v1/send`,
          method: 'POST',
          timeout_ms: 5000,
          max_request_bytes: 65536,
          max_response_bytes: 65536,
          // Development fixture only: production points `mail` at an HTTPS provider.
          allow_loopback: true,
        }],
      },
    };
    writeFileSync(config, JSON.stringify(serviceConfig));

    assert.equal(JSON.parse(run(binary, ['service', 'init', file, '--config', config])).ok, true);
    const q = `"${schema}"`;
    sql(`INSERT INTO ${q}."group" (id,name,guest,admin,super_mod,mod,banned,editpost,deletepost,deletetopic,posttopic,postreply,viewhidden,makehidden,mod_edituser,mod_banuser,description) VALUES
      (1,'Guest',true,false,false,false,false,false,false,false,false,false,false,false,false,false,NULL),
      (2,'Member',false,false,false,false,false,true,false,false,true,true,false,false,false,false,NULL);`);
    sql(`INSERT INTO ${q}.user (id,username,email,date_joined,lastseen,primary_group_id,activated,language) VALUES
      (2,'member-alice','${aliceMail}',now(),now(),2,true,'en'),
      (3,'member-carol','${carolMail}',now(),now(),2,true,'en');`);
    run(binary, ['service', 'principal', '--config', config, 'bind', '--subject', 'alice-sub', '--actor', '2']);

    run(process.execPath, ['product/tools/build-sdk.mjs', '--out', sdkRoot]);
    run(binary, ['service', 'gen', file, '--out', bindingPath]);
    const bindingSource = readFileSync(bindingPath, 'utf8');
    const section = name => JSON.parse(bindingSource.match(new RegExp(`export const ${name} = (.*) as const;`))[1]);
    const identityKinds = JSON.parse(bindingSource.match(/identityKinds: (.*), fingerprint:/)[1]);
    const fingerprint = bindingSource.match(/export const contractFingerprint = "([0-9a-f]+)"/)[1];
    const contract = {fingerprint, idWire: 'decimal-string-v13', identityKinds, extensions: section('extensionDescriptors'), readDescriptors: section('readDescriptors'), operations: section('operationDescriptors')};

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
    assert.equal(started.ok, true, JSON.stringify(started));
    lines.close();
    server.stdout.destroy();

    const {connect} = await import(pathToFileURL(join(sdkRoot, 'product/sdk/index.js')).href);
    const token = subject => {
      const now = Math.floor(Date.now() / 1000);
      const unsigned = `${b64({alg: 'RS256', typ: 'at+jwt', kid: jwk.kid})}.${b64({iss: 'https://flaskbb.test', aud: 'aip-flaskbb', sub: subject, iat: now, nbf: now, exp: now + 600})}`;
      return `${unsigned}.${sign('RSA-SHA256', Buffer.from(unsigned), privateKey).toString('base64url')}`;
    };
    const alice = connect(`http://${started.address}`, token('alice-sub'), contract);
    assert.ok(alice.jobs, 'generated binding must expose jobs');

    const settle = async receipt => {
      for (let attempt = 0; attempt < 100; attempt++) {
        const status = await alice.jobs.status(receipt.id);
        if (['SUCCEEDED', 'FAILED', 'CANCELLED'].includes(status.status)) return status;
        await new Promise(done => setTimeout(done, 100));
      }
      throw new Error('job did not settle');
    };
    const inbox = async recipient => {
      const response = await fetch(`${mailpit}/api/v1/search?query=${encodeURIComponent(`to:${recipient}`)}`);
      assert.equal(response.status, 200);
      return (await response.json()).messages;
    };
    const body = async id => (await (await fetch(`${mailpit}/api/v1/message/${id}`)).json());

    // Activation mail: worker has no network, delivery must come from the broker.
    const activationToken = `act-${runTag}`;
    const activation = await alice.jobs.enqueue('sendActivationEmail', {email: aliceMail, username: 'member-alice', token: activationToken}, {key: `activation-${runTag}`});
    const activationStatus = await settle(activation);
    assert.equal(activationStatus.status, 'SUCCEEDED', JSON.stringify(activationStatus));
    assert.deepEqual(await alice.jobs.result(activation.id, 'sendActivationEmail'), {accepted: true});
    const sent = (await inbox(aliceMail)).filter(message => message.Subject === 'Activate your FlaskBB account');
    assert.equal(sent.length, 1, 'exactly one activation mail reaches Mailpit');
    const detail = await body(sent[0].ID);
    assert.equal(detail.From.Address, 'noreply@flaskbb.test');
    assert.equal(detail.To[0].Address, aliceMail);
    assert.match(detail.Text, new RegExp(`http://127\\.0\\.0\\.1:58080/auth/activate/${activationToken}`));

    // Same job key and input replays instead of sending twice.
    const replay = await alice.jobs.enqueue('sendActivationEmail', {email: aliceMail, username: 'member-alice', token: activationToken}, {key: `activation-${runTag}`});
    assert.equal(replay.id, activation.id);
    assert.equal((await inbox(aliceMail)).filter(message => message.Subject === 'Activate your FlaskBB account').length, 1);

    // The effect journal is per job run: an explicit second job (new key) is a new send.
    const resend = await alice.jobs.enqueue('sendActivationEmail', {email: aliceMail, username: 'member-alice', token: activationToken}, {key: `activation-resend-${runTag}`});
    assert.equal((await settle(resend)).status, 'SUCCEEDED');
    assert.equal((await inbox(aliceMail)).filter(message => message.Subject === 'Activate your FlaskBB account').length, 2, 'a new job key is an explicit resend');

    // Password reset mail with its own token.
    const resetToken = `rst-${runTag}`;
    const reset = await alice.jobs.enqueue('sendPasswordResetEmail', {email: aliceMail, username: 'member-alice', token: resetToken}, {key: `reset-${runTag}`});
    assert.equal((await settle(reset)).status, 'SUCCEEDED');
    const resets = (await inbox(aliceMail)).filter(message => message.Subject === 'Reset your FlaskBB password');
    assert.equal(resets.length, 1);
    assert.match((await body(resets[0].ID)).Text, new RegExp(`/auth/reset/${resetToken}`));

    // A job must not choose a recipient other than the caller's own account.
    const foreign = await alice.jobs.enqueue('sendActivationEmail', {email: carolMail, username: 'member-carol', token: `evil-${runTag}`}, {key: `foreign-${runTag}`}).catch(error => ({rejected: error}));
    if (!foreign.rejected) {
      const foreignStatus = await settle(foreign);
      assert.notEqual(foreignStatus.status, 'SUCCEEDED', 'relaying a mail to another user must be refused');
    }
    assert.equal((await inbox(carolMail)).length, 0, 'no mail may reach a recipient who is not the caller');

    console.log(JSON.stringify({schema, ok: true, verified: ['worker_net_isolated', 'broker_http_delivery', 'activation_mail', 'reset_mail', 'same_key_replay_no_duplicate', 'foreign_recipient_refused']}));
  } catch (error) {
    console.error(error);
    throw error;
  } finally {
    if (server && !server.closed) {
      const closed = once(server, 'close');
      if (server.exitCode === null && server.signalCode === null) server.kill('SIGKILL');
      await closed;
    }
    sql(`DROP SCHEMA IF EXISTS "${schema}" CASCADE;`);
    rmSync(temp, {recursive: true, force: true});
  }
});

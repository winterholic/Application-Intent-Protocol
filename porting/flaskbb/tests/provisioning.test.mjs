import {dbUrl} from './harness.mjs';
import assert from 'node:assert/strict';
import {test} from 'node:test';
import {generateKeyPairSync, sign, randomUUID} from 'node:crypto';
import {mkdtempSync, readFileSync, writeFileSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join, resolve} from 'node:path';
import {once} from 'node:events';
import {createInterface} from 'node:readline';
import {pathToFileURL} from 'node:url';
import {spawn, spawnSync} from 'node:child_process';

const track = resolve(new URL('..', import.meta.url).pathname);
const root = resolve(track, '../..');
const binary = process.env.AIP_BINARY || join(root, 'target/debug/aip');
const file = join(track, 'app.aip');
const b64 = value => Buffer.from(JSON.stringify(value)).toString('base64url');

test('AIP core provisioning working proposal (contract verification in real PostgreSQL)', {timeout: 90000}, async () => {
  const temp = mkdtempSync(join(tmpdir(), 'aip-flaskbb-provision-'));
  const suffix = randomUUID().replaceAll('-', '').slice(0, 10);
  const schema = `aip_flaskbb_${suffix}`;
  const env = {...process.env, AIP_FLASKBB_DATABASE: dbUrl};

  const run = (command, args, options = {}) => {
    const result = spawnSync(command, args, {cwd: root, env, encoding: 'utf8', timeout: 20000, maxBuffer: 4 << 20, ...options});
    assert.equal(result.status, 0, `${command} ${args.join(' ')}\n${result.stdout}\n${result.stderr}`);
    return result.stdout.trim();
  };

  const sql = statement => run('psql', ['-X', '-v', 'ON_ERROR_STOP=1', '-At', '-d', dbUrl, '-c', statement]);

  const configPath = join(temp, 'service.json');
  const configDisabledPath = join(temp, 'service-disabled.json');
  const sdkRoot = join(temp, 'sdk');
  const jwksPath = join(temp, 'jwks.json');
  const bindingPath = join(temp, 'binding.ts');
  let server;
  let serverDisabled;

  try {
    const {publicKey, privateKey} = generateKeyPairSync('rsa', {modulusLength: 2048});
    const jwk = {...publicKey.export({format: 'jwk'}), kid: 'provision-fixture', alg: 'RS256', use: 'sig', key_ops: ['verify']};
    writeFileSync(jwksPath, JSON.stringify({keys: [jwk]}));

    const serviceConfig = {
      schema,
      database_url_env: 'AIP_FLASKBB_DATABASE',
      migrate_database_url_env: 'AIP_FLASKBB_DATABASE',
      listen: '127.0.0.1:0',
      wire: 'decimal',
      allowed_origins: ['http://127.0.0.1:58080'],
      auth: {
        issuer: 'https://flaskbb.test',
        audience: 'aip-flaskbb',
        jwks: {kind: 'file', path: jwksPath},
        provisioning: {
          enabled: true,
          actor_resource: 'User',
          username_claim: 'preferred_username',
          email_claim: 'email',
          require_email_verified: true,
          fixed_defaults: {
            primary_group_name: 'Member',
            activated: true
          }
        }
      }
    };
    writeFileSync(configPath, JSON.stringify(serviceConfig));

    const serviceConfigDisabled = {
      ...serviceConfig,
      auth: {
        issuer: 'https://flaskbb.test',
        audience: 'aip-flaskbb',
        jwks: {kind: 'file', path: jwksPath},
        provisioning: {
          enabled: false
        }
      }
    };
    writeFileSync(configDisabledPath, JSON.stringify(serviceConfigDisabled));

    const init = JSON.parse(run(binary, ['service', 'init', file, '--config', configPath]));
    assert.equal(init.ok, true);

    run(process.execPath, ['product/tools/build-sdk.mjs', '--out', sdkRoot]);

    const q = `"${schema}"`;
    sql(`INSERT INTO ${q}."group" (id,name,guest,admin,super_mod,mod,banned,editpost,deletepost,deletetopic,posttopic,postreply,viewhidden,makehidden,mod_edituser,mod_banuser,description) VALUES
      (1,'Guest',true,false,false,false,false,false,false,false,false,false,false,false,false,false,NULL),
      (2,'Member',false,false,false,false,false,true,false,false,true,true,false,false,false,false,NULL),
      (3,'Administrator',false,true,false,false,false,true,true,true,true,true,true,true,true,true,NULL);`);

    sql(`INSERT INTO ${q}.category (id,title,description,position) VALUES (1,'General','Fixture',1);`);
    sql(`INSERT INTO ${q}.forum (id,category_id,title,description,position,locked,external,show_moderators,last_post_id,last_post_user_id,last_post_title,last_post_username,last_post_created) VALUES
      (1,1,'Public forum','Visible to members and guests',1,false,NULL,false,NULL,NULL,NULL,NULL,NULL);`);
    sql(`INSERT INTO ${q}.forum_group_access (id,forum_id,group_id) VALUES (1,1,1),(2,1,2),(3,1,3);`);

    const generated = JSON.parse(run(binary, ['service', 'gen', file, '--out', bindingPath]));
    const bindingSource = readFileSync(bindingPath, 'utf8');
    const descriptors = JSON.parse(bindingSource.match(/export const readDescriptors = (.*) as const;/)[1]);
    const identityKinds = JSON.parse(bindingSource.match(/identityKinds: (.*), fingerprint:/)[1]);
    const contract = {fingerprint: generated.fingerprint, idWire: 'decimal-string-v13', identityKinds, extensions: {}, readDescriptors: descriptors};

    // Start server with provisioning enabled
    server = spawn(binary, ['service', 'serve', file, '--config', configPath], {cwd: temp, env, stdio: ['ignore', 'pipe', 'pipe']});
    let stderr = '';
    server.stderr.on('data', data => {stderr += data;});
    const lines = createInterface({input: server.stdout});
    const started = await Promise.race([
      once(lines, 'line').then(([line]) => JSON.parse(line)),
      once(server, 'exit').then(([code]) => {throw new Error(`serve exit ${code}: ${stderr}`);}),
      new Promise((_, reject) => setTimeout(() => reject(new Error(`serve timeout: ${stderr}`)), 15000).unref())
    ]);
    assert.equal(started.ok, true, JSON.stringify(started));

    const token = (claims = {}) => {
      const now = Math.floor(Date.now() / 1000);
      const payload = {
        iss: 'https://flaskbb.test',
        aud: 'aip-flaskbb',
        iat: now,
        nbf: now,
        exp: now + 600,
        ...claims
      };
      const unsigned = `${b64({alg: 'RS256', typ: 'at+jwt', kid: jwk.kid})}.${b64(payload)}`;
      return `${unsigned}.${sign('RSA-SHA256', Buffer.from(unsigned), privateKey).toString('base64url')}`;
    };

    const callSession = async (t, address = started.address) => {
      const res = await fetch(`http://${address}/session`, {
        method: 'POST',
        headers: {
          'content-type': 'application/json',
          ...(t ? {'authorization': `Bearer ${t}`} : {})
        },
        body: JSON.stringify({})
      });
      const data = await res.json();
      return {status: res.status, data};
    };

    const {connect} = await import(pathToFileURL(join(sdkRoot, 'product/sdk/index.js')).href);

    // =========================================================================
    // 1. Positive: 신규 주체 최초 세션 수립 및 후속 SDK 읽기
    // =========================================================================
    const aliceToken = token({
      sub: 'alice-sub-001',
      preferred_username: 'alice',
      email: 'alice@example.test',
      email_verified: true
    });

    const aliceSession = await callSession(aliceToken);
    assert.equal(aliceSession.status, 200);
    assert.equal(aliceSession.data.ok, true);
    assert.ok(aliceSession.data.principal?.actorId);
    const aliceActorId = aliceSession.data.principal.actorId;

    // DB 검증: User row 1건, primary_group = Member(2), activated = true
    const userRow = sql(`SELECT id, username, email, primary_group_id, activated FROM ${q}."user" WHERE username='alice'`);
    assert.equal(userRow, `${aliceActorId}|alice|alice@example.test|2|t`);

    // DB 검증: aip_principals 1건, actor_id 일치, enabled=true
    const principalRow = sql(`SELECT issuer, subject, actor_id, enabled FROM ${q}.aip_principals WHERE subject='alice-sub-001'`);
    assert.equal(principalRow, `https://flaskbb.test|alice-sub-001|${aliceActorId}|t`);

    // 후속 요청: 동일 토큰으로 SDK connect 및 읽기 동작 확인
    const aliceClient = connect(`http://${started.address}`, aliceToken, contract);
    const categoryRead = await aliceClient.read({read: 'Category', select: ['id', 'title'], sort: [{field: 'id', dir: 'asc'}], limit: 10});
    assert.equal(categoryRead.rows.length, 1);
    assert.equal(categoryRead.rows[0].title, 'General');

    const forumRead = await aliceClient.read({read: 'Forum', select: ['id', 'title'], sort: [{field: 'id', dir: 'asc'}], limit: 10});
    assert.equal(forumRead.rows.length, 1);
    assert.equal(forumRead.rows[0].title, 'Public forum');

    // =========================================================================
    // 2. Negative: 비세션 경로 프로비저닝 차단
    // =========================================================================
    const unregisteredToken = token({
      sub: 'unregistered-sub-999',
      preferred_username: 'unregistered',
      email: 'unregistered@example.test',
      email_verified: true
    });

    // POST /session을 거치지 않고 직접 POST /read 호출
    const directReadRes = await fetch(`http://${started.address}/read`, {
      method: 'POST',
      headers: {
        'content-type': 'application/json',
        'authorization': `Bearer ${unregisteredToken}`,
        'x-aip-contract': contract.fingerprint
      },
      body: JSON.stringify({read: 'Category', select: ['id', 'title'], limit: 10})
    });
    const directReadData = await directReadRes.json();
    assert.equal(directReadData.ok, false);
    assert.equal(directReadData.code, 'WIRE.UNAUTHENTICATED');

    // DB 확인: unregistered 사용자는 생성되지 않아야 함
    const unregisteredUser = sql(`SELECT count(*) FROM ${q}."user" WHERE username='unregistered'`);
    assert.equal(unregisteredUser, '0');
    const unregisteredPrincipal = sql(`SELECT count(*) FROM ${q}.aip_principals WHERE subject='unregistered-sub-999'`);
    assert.equal(unregisteredPrincipal, '0');

    // auth.provisioning.enabled = false 배포에서는 POST /session도 즉시 거부
    serverDisabled = spawn(binary, ['service', 'serve', file, '--config', configDisabledPath], {cwd: temp, env, stdio: ['ignore', 'pipe', 'pipe']});
    const disabledLines = createInterface({input: serverDisabled.stdout});
    const disabledStarted = await Promise.race([
      once(disabledLines, 'line').then(([line]) => JSON.parse(line)),
      once(serverDisabled, 'exit').then(([code]) => {throw new Error(`disabled serve exit ${code}`);}),
      new Promise((_, reject) => setTimeout(() => reject(new Error('timeout')), 15000).unref())
    ]);
    assert.equal(disabledStarted.ok, true);

    const disabledSession = await callSession(unregisteredToken, disabledStarted.address);
    assert.equal(disabledSession.data.ok, false);
    assert.equal(disabledSession.data.code, 'WIRE.UNAUTHENTICATED');

    // =========================================================================
    // 3. Negative: 미검증 이메일 및 기존 계정 충돌/병합 차단
    // =========================================================================
    // 3a. email_verified: false
    const unverifiedEmailToken = token({
      sub: 'unverified-email-sub',
      preferred_username: 'unverified_user',
      email: 'unverified@example.test',
      email_verified: false
    });
    const unverifiedSession = await callSession(unverifiedEmailToken);
    assert.equal(unverifiedSession.data.ok, false);
    assert.equal(unverifiedSession.data.code, 'WIRE.UNAUTHENTICATED');
    assert.equal(sql(`SELECT count(*) FROM ${q}."user" WHERE username='unverified_user'`), '0');
    assert.equal(sql(`SELECT count(*) FROM ${q}.aip_principals WHERE subject='unverified-email-sub'`), '0');

    // 3b. email_verified 누락
    const missingEmailVerifiedToken = token({
      sub: 'missing-verified-sub',
      preferred_username: 'missing_verified',
      email: 'missing@example.test'
    });
    const missingSession = await callSession(missingEmailVerifiedToken);
    assert.equal(missingSession.data.ok, false);
    assert.equal(missingSession.data.code, 'WIRE.UNAUTHENTICATED');
    assert.equal(sql(`SELECT count(*) FROM ${q}."user" WHERE username='missing_verified'`), '0');
    assert.equal(sql(`SELECT count(*) FROM ${q}.aip_principals WHERE subject='missing-verified-sub'`), '0');

    // 3c. 기존 username("alice")과 충돌하는 신규 주체
    const conflictUsernameToken = token({
      sub: 'attacker-sub-user-conflict',
      preferred_username: 'alice',
      email: 'attacker@example.test',
      email_verified: true
    });
    const conflictUserSession = await callSession(conflictUsernameToken);
    assert.equal(conflictUserSession.data.ok, false);
    assert.equal(conflictUserSession.data.code, 'WIRE.UNAUTHENTICATED');

    // 3d. 기존 email("alice@example.test")과 충돌하는 신규 주체
    const conflictEmailToken = token({
      sub: 'attacker-sub-email-conflict',
      preferred_username: 'attacker_user',
      email: 'alice@example.test',
      email_verified: true
    });
    const conflictEmailSession = await callSession(conflictEmailToken);
    assert.equal(conflictEmailSession.data.ok, false);
    assert.equal(conflictEmailSession.data.code, 'WIRE.UNAUTHENTICATED');

    // 3e. 잘못된 형식의 이메일 (malformed email) 차단
    for (const badEmail of ['not-an-email@', 'not-an-email', 'user@.test', 'user@@example.test', 'user@example.test ', 'user\u0000@example.test']) {
      const malformedEmailToken = token({
        sub: `bad-email-sub-${randomUUID().slice(0, 8)}`,
        preferred_username: `bad_email_${randomUUID().slice(0, 8)}`,
        email: badEmail,
        email_verified: true
      });
      const malformedSession = await callSession(malformedEmailToken);
      assert.equal(malformedSession.data.ok, false, `Expected rejection for bad email: ${badEmail}`);
      assert.equal(malformedSession.data.code, 'WIRE.UNAUTHENTICATED');
    }
    const badEmailUserCount = sql(`SELECT count(*) FROM ${q}."user" WHERE email LIKE 'not-an-email%' OR email LIKE 'user@%'`);
    assert.equal(badEmailUserCount, '0');

    // Missing and explicit-null email claims are distinct malformed inputs;
    // neither may create an actor or principal.
    for (const [label, emailClaim] of [['missing', undefined], ['null', null]]) {
      const subject = `bad-email-${label}-${randomUUID().slice(0, 8)}`;
      const username = `bad_email_${label}_${randomUUID().slice(0, 8)}`;
      const claims = {
        sub: subject,
        preferred_username: username,
        email_verified: true,
      };
      if (emailClaim !== undefined) claims.email = emailClaim;
      const response = await callSession(token(claims));
      assert.equal(response.data.ok, false, `${label} email claim must be rejected`);
      assert.equal(response.data.code, 'WIRE.UNAUTHENTICATED');
      assert.equal(sql(`SELECT count(*) FROM ${q}.aip_principals WHERE subject='${subject}'`), '0');
      assert.equal(sql(`SELECT count(*) FROM ${q}."user" WHERE username='${username}'`), '0');
    }

    // 3f. email_verified 타입 경계 (null, 문자열 "true") 차단
    for (const badVerified of [null, 'true', 1]) {
      const subject = `bad-verified-sub-${randomUUID().slice(0, 8)}`;
      const username = `bad_verified_${randomUUID().slice(0, 8)}`;
      const badVerifiedToken = token({
        sub: subject,
        preferred_username: username,
        email: `${username}@example.test`,
        email_verified: badVerified
      });
      const badVerifiedSession = await callSession(badVerifiedToken);
      assert.equal(badVerifiedSession.data.ok, false, `Expected rejection for bad email_verified: ${badVerified}`);
      assert.equal(badVerifiedSession.data.code, 'WIRE.UNAUTHENTICATED');
      assert.equal(sql(`SELECT count(*) FROM ${q}.aip_principals WHERE subject='${subject}'`), '0');
      assert.equal(sql(`SELECT count(*) FROM ${q}."user" WHERE username='${username}'`), '0');
    }

    // alice의 aip_principals가 attacker에게 연결되거나 변경되지 않았음을 확인
    const attackerPrincipal = sql(`SELECT count(*) FROM ${q}.aip_principals WHERE subject LIKE 'attacker-sub%'`);
    assert.equal(attackerPrincipal, '0');

    // =========================================================================
    // 4. Negative: IdP 권한 클레임 주입(권한 상승) 차단
    // =========================================================================
    const forgedRoleToken = token({
      sub: 'mallory-sub-002',
      preferred_username: 'mallory',
      email: 'mallory@example.test',
      email_verified: true,
      roles: ['Administrator'],
      groups: ['Administrator'],
      primary_group_id: 3,
      actor_id: 3,
      primary_group: 3
    });
    const mallorySession = await callSession(forgedRoleToken);
    assert.equal(mallorySession.status, 200);
    assert.equal(mallorySession.data.ok, true);
    const malloryActorId = mallorySession.data.principal.actorId;
    assert.notEqual(malloryActorId, '3', '새 actorId가 발급되어야 하며 클라이언트 지정 actorId는 무시되어야 함');

    // DB 확인: mallory의 primary_group은 서버 고정 Member(2)여야 함
    const malloryGroup = sql(`SELECT primary_group_id FROM ${q}."user" WHERE username='mallory'`);
    assert.equal(malloryGroup, '2', '서버 설정 Member 그룹이어야 하며 토큰의 관리자 권한 클레임은 무시되어야 함');

    // =========================================================================
    // 5. Negative: 탈퇴·정지·폐기(enabled=false / min_iat) 후 자동 재생성 차단
    // =========================================================================
    // alice를 enabled=false (정지/탈퇴)로 변경
    sql(`UPDATE ${q}.aip_principals SET enabled=false WHERE subject='alice-sub-001'`);
    const aliceDisabledSession = await callSession(aliceToken);
    assert.equal(aliceDisabledSession.data.ok, false);
    assert.equal(aliceDisabledSession.data.code, 'WIRE.UNAUTHENTICATED');

    // mallory의 min_iat를 미래로 설정하여 토큰 폐기
    const futureIat = Math.floor(Date.now() / 1000) + 10000;
    sql(`UPDATE ${q}.aip_principals SET min_iat=${futureIat} WHERE subject='mallory-sub-002'`);
    const malloryRevokedSession = await callSession(forgedRoleToken);
    assert.equal(malloryRevokedSession.data.ok, false);
    assert.equal(malloryRevokedSession.data.code, 'WIRE.UNAUTHENTICATED');

    // =========================================================================
    // 6. Concurrency: 동시 10개 세션 수립 요청 경합 및 멱등성
    // =========================================================================
    const concurrentToken = token({
      sub: 'concurrent-sub-003',
      preferred_username: 'concurrent_user',
      email: 'concurrent@example.test',
      email_verified: true
    });

    const concurrentResults = await Promise.all(
      Array.from({length: 10}, () => callSession(concurrentToken))
    );

    const firstActorId = concurrentResults[0].data.principal.actorId;
    for (const res of concurrentResults) {
      assert.equal(res.status, 200);
      assert.equal(res.data.ok, true);
      assert.equal(res.data.principal.actorId, firstActorId, '모든 동시 요청이 동일한 단일 actorId를 반환해야 함');
    }

    const concurrentUserCount = sql(`SELECT count(*) FROM ${q}."user" WHERE username='concurrent_user'`);
    assert.equal(concurrentUserCount, '1', '동시 경합에도 user 테이블에는 정확히 1개의 행만 생성되어야 함');

    const concurrentPrincipalCount = sql(`SELECT count(*) FROM ${q}.aip_principals WHERE subject='concurrent-sub-003'`);
    assert.equal(concurrentPrincipalCount, '1', 'aip_principals 테이블에는 정확히 1개의 행만 생성되어야 함');

    // =========================================================================
    // 7. Rollback: 원자성 검증 (충돌 시 고아 row 잔존 차단)
    // =========================================================================
    const userCountBefore = parseInt(sql(`SELECT count(*) FROM ${q}."user"`), 10);
    const principalCountBefore = parseInt(sql(`SELECT count(*) FROM ${q}.aip_principals`), 10);

    // email이 기존 concurrent@example.test와 충돌하는 요청
    const failToken = token({
      sub: 'fail-rollback-sub',
      preferred_username: 'will_fail',
      email: 'concurrent@example.test',
      email_verified: true
    });
    const failSession = await callSession(failToken);
    assert.equal(failSession.data.ok, false);

    const userCountAfter = parseInt(sql(`SELECT count(*) FROM ${q}."user"`), 10);
    const principalCountAfter = parseInt(sql(`SELECT count(*) FROM ${q}.aip_principals`), 10);

    assert.equal(userCountAfter, userCountBefore, '롤백 후 user 테이블 행 수가 증가하지 않아야 함');
    assert.equal(principalCountAfter, principalCountBefore, '롤백 후 aip_principals 테이블 행 수가 증가하지 않아야 함');

    console.log(JSON.stringify({
      schema,
      ok: true,
      tests: [
        'positive_first_session_and_reads',
        'negative_non_session_route_blocked',
        'negative_provisioning_disabled_blocked',
        'negative_unverified_email_blocked',
        'negative_username_email_conflict_no_merge',
        'negative_client_role_injection_ignored',
        'negative_disabled_revoked_no_recreation',
        'concurrency_10_parallel_sessions_idempotent',
        'rollback_atomicity_no_orphan_rows'
      ]
    }));
  } finally {
    if (server) {
      server.kill('SIGTERM');
    }
    if (serverDisabled) {
      serverDisabled.kill('SIGTERM');
    }
    sql(`DROP SCHEMA IF EXISTS "${schema}" CASCADE;`);
  }
});

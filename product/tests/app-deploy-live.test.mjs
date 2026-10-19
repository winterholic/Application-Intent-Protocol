// This test owns a disposable PostgreSQL container. No operator DB or default DSN is used.
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { generateKeyPairSync, randomUUID, sign } from "node:crypto";
import { chmod, mkdir, mkdtemp, readFile, realpath, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createServer } from "node:net";
import { fileURLToPath } from "node:url";
import { once } from "node:events";
import { test } from "node:test";

const repo = fileURLToPath(new URL("../../", import.meta.url));
function run(program, args, { cwd = repo, env = process.env, input } = {}) {
  const result = spawnSync(program, args, { cwd, env, input, encoding: "utf8", timeout: 1_200_000, maxBuffer: 8 * 1024 * 1024 });
  assert.equal(result.error, undefined, result.error?.message);
  assert.equal(result.status, 0, `${program} failed: ${result.stderr}\n${result.stdout}`);
  return result.stdout.trim();
}
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
async function freePort() {
  const server = createServer(); await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  const port = server.address().port; await new Promise(resolve => server.close(resolve)); return port;
}

test("moved app package initializes, serves HTTP, stops, migrates by digest and preserves data", { timeout: 1_800_000 }, async () => {
  const base = await realpath(await mkdtemp(join(tmpdir(), "aip-app-deploy-live-")));
  const container = `aip-app-deploy-${randomUUID().slice(0, 8)}`;
  let owned = false; let server;
  let image; let imageContainer;
  const stop = async () => {
    if (!server || server.exitCode !== null || server.signalCode !== null) return;
    const stopped = once(server, "exit"); server.kill("SIGTERM");
    const timer = setTimeout(() => server.kill("SIGKILL"), 5000);
    try { const [code, signal] = await stopped; assert.equal(signal, null, "supervisor must reap child without forced kill"); assert.equal(code, 0); }
    finally { clearTimeout(timer); }
  };
  try {
    run("docker", ["run", "--detach", "--rm", "--name", container, "-e", "POSTGRES_HOST_AUTH_METHOD=trust", "-e", "POSTGRES_DB=aip_test_app_deploy", "-p", "127.0.0.1::5432", "postgres:17"]); owned = true;
    let ready = false;
    for (let attempt = 0; attempt < 100; attempt++) {
      const result = spawnSync("docker", ["exec", container, "pg_isready", "-U", "postgres"], { stdio: "ignore" });
      if (result.status === 0) { ready = true; break; } await sleep(100);
    }
    assert.equal(ready, true, "owned PostgreSQL must be ready");
    const port = run("docker", ["port", container, "5432/tcp"]).split(":").at(-1);
    const env = { ...process.env, AIP_DEPLOY_TEST_DSN: `host=127.0.0.1 port=${port} dbname=aip_test_app_deploy user=postgres` };
    const sql = statement => run("docker", ["exec", "-i", container, "psql", "-X", "-v", "ON_ERROR_STOP=1", "-U", "postgres", "-d", "aip_test_app_deploy", "-At"], { input: statement });
    const definition = 'resource Member { fields { id: Id } }\nactor Member\nresource Item { fields { id: Id; owner: Member; checked: Bool } rows read when owner = actor\ntransition mark { allow owner = actor; from checked = false; to checked = true; repeat unchanged }\nexpose read { select id, checked; sort id; budget { rows 20; depth 1; deadline 1s; cost 100 } }\nexpose apply mark { target id; bulk maxRows 1 } }\n';
    const bundles = [];
    for (const version of ["1.0.0", "1.1.0"]) {
      const input = join(base, `input-${version}`); const output = join(base, `bundle-${version}`);
      await mkdir(input); await writeFile(join(input, "app.aip"), version === "1.0.0" ? definition : definition.replace("checked: Bool", "checked: Bool; note: Text?"));
      await writeFile(join(input, "aip.app.json"), JSON.stringify({ name: "deploy-live", version, definition: "app.aip" }));
      run(process.execPath, [join(repo, "product/tools/package.mjs"), "--manifest", input, "--out", output]);
      const installed = join(base, `installed-${version}`);
      run(process.execPath, [join(output, "tools/app-deploy.mjs"), "install", "--out", installed], { cwd: base });
      await rm(input, { recursive: true }); await rm(output, { recursive: true });
      bundles.push(installed);
    }
    const configPath = join(base, "operator.json");
    const listen = `127.0.0.1:${await freePort()}`;
    const { privateKey, publicKey } = generateKeyPairSync("rsa", { modulusLength: 2048 });
    const jwk = { ...publicKey.export({ format: "jwk" }), kid: "deploy-test", use: "sig", alg: "RS256" };
    await writeFile(join(base, "jwks.json"), JSON.stringify({ keys: [jwk] }));
    await writeFile(configPath, JSON.stringify({ schema: "aip_deploy_live", database_url_env: "AIP_DEPLOY_TEST_DSN", listen, wire: "decimal", auth: { issuer: "https://issuer.example", audience: "aip-api", jwks: { kind: "file", path: "jwks.json" } } }));
    const command = (bundle, action, extra = []) => JSON.parse(run(process.execPath, [join(bundle, "tools/app-deploy.mjs"), action, "--config", configPath, ...extra], { cwd: base, env }));
    const start = async bundle => {
      server = spawn(process.execPath, [join(bundle, "tools/app-deploy.mjs"), "serve", "--config", configPath], { cwd: base, env, stdio: ["ignore", "pipe", "pipe"] });
      let output = ""; server.stdout.on("data", bytes => { output += bytes; }); server.stderr.on("data", bytes => { output += bytes; });
      for (let attempt = 0; attempt < 150; attempt++) {
        assert.equal(server.exitCode, null, output);
        try { if ((await fetch(`http://${listen}/ready`)).status === 200) return; } catch {}
        await sleep(100);
      }
      assert.fail(`service startup timed out: ${output}`);
    };
    command(bundles[0], "preflight"); command(bundles[0], "init");
    sql("INSERT INTO aip_deploy_live.member(id) VALUES (42); INSERT INTO aip_deploy_live.item(id, owner_id, checked) VALUES (11,42,false);");
    run(join(bundles[0], "bin/aip"), ["service", "principal", "--config", configPath, "bind", "--subject", "user-42", "--actor", "42"], { env });
    const tokenPart = value => Buffer.from(JSON.stringify(value)).toString("base64url");
    const unsigned = `${tokenPart({ alg: "RS256", typ: "at+jwt", kid: jwk.kid })}.${tokenPart({ iss: "https://issuer.example", aud: "aip-api", sub: "user-42", iat: Math.floor(Date.now()/1000), exp: Math.floor(Date.now()/1000)+300 })}`;
    const token = `${unsigned}.${sign("RSA-SHA256", Buffer.from(unsigned), privateKey).toString("base64url")}`;
    const fingerprint = bundle => JSON.parse(run(join(bundle, "bin/aip"), ["service", "gen", join(bundle, "app/app.aip"), "--out", join(base, "bindings.ts")])).fingerprint;
    let contract = fingerprint(bundles[0]);
    const call = async (path, body) => (await fetch(`http://${listen}${path}`, { method: "POST", headers: { "content-type": "application/json", authorization: `Bearer ${token}`, "x-aip-contract": contract }, body: JSON.stringify(body) })).json();
    await start(bundles[0]);
    assert.equal(command(bundles[0], "health").ok, true); assert.equal(command(bundles[0], "ready").ok, true);
    assert.equal((await call("/read", { query: { read: "Item", select: ["id", "checked"] } })).rows[0].checked, false);
    assert.equal((await call("/apply", { request: { apply: "Item.mark", target: { ids: ["11"] } }, key: "deploy-write-1" })).ok, true);
    await stop();
    await assert.rejects(fetch(`http://${listen}/ready`));
    const backup = run("docker", ["exec", container, "pg_dump", "-U", "postgres", "-d", "aip_test_app_deploy", "--clean", "--if-exists", "--no-owner", "--no-privileges"]);
    const plan = JSON.parse(command(bundles[1], "plan").output);
    assert.equal(plan.blocked, false); assert.match(plan.digest, /^[a-f0-9]{64}$/u);
    const rejected = spawnSync(process.execPath, [join(bundles[1], "tools/app-deploy.mjs"), "migrate", "--config", configPath, "--digest", "0".repeat(64)], { env, encoding: "utf8" });
    assert.notEqual(rejected.status, 0); assert.equal(sql("SELECT checked FROM aip_deploy_live.item WHERE id=11;"), "t");
    command(bundles[1], "migrate", ["--digest", plan.digest]);
    contract = fingerprint(bundles[1]); await start(bundles[1]);
    assert.equal((await call("/read", { query: { read: "Item", select: ["id", "checked"] } })).rows[0].checked, true);
    assert.equal((await call("/apply", { request: { apply: "Item.mark", target: { ids: ["11"] } }, key: "deploy-write-1" })).replayed, true);
    await stop(); await start(bundles[1]); assert.equal(command(bundles[1], "ready").ok, true); await stop();
    const oldVersion = spawnSync(process.execPath, [join(bundles[0], "tools/app-deploy.mjs"), "serve", "--config", configPath], { cwd: base, env, encoding: "utf8", timeout: 5000 });
    assert.equal(oldVersion.error, undefined, "old definition must be rejected, not hang serving changed schema");
    assert.notEqual(oldVersion.status, 0); assert.match(oldVersion.stdout + oldVersion.stderr, /SCHEMA_MISMATCH/u);
    console.log("relocated package: init, authenticated HTTP read/apply, SIGTERM, restart, wrong digest refusal, migration, preserved row/idempotency PASS");
    if (process.platform === "linux" && process.env.AIP_TEST_APP_DOCKER === "1") {
      image = `aip-app-deploy-test:${randomUUID().slice(0, 8)}`;
      run("docker", ["build", "--tag", image, "--file", join(bundles[1], "deploy/Dockerfile"), bundles[1]]);
      const metadata = JSON.parse(run("docker", ["image", "inspect", image]))[0];
      assert.equal(metadata.Config.User, "10001:10001");
      await chmod(base, 0o755);
      const operator = join(base, "operator"); await mkdir(operator);
      await writeFile(join(operator, "service.json"), await readFile(configPath));
      await writeFile(join(operator, "jwks.json"), await readFile(join(base, "jwks.json")));
      for (let cycle = 0; cycle < 2; cycle++) {
        imageContainer = `aip-app-image-${randomUUID().slice(0, 8)}`;
        run("docker", ["run", "--detach", "--name", imageContainer, "--init", "--network", "host", "--read-only", "--cap-drop", "ALL", "--security-opt", "no-new-privileges:true", "--tmpfs", "/tmp:rw,nosuid,nodev,size=64m", "--mount", `type=bind,source=${operator},target=/run/aip,readonly`, "--env", "AIP_DEPLOY_TEST_DSN", image], { env });
        let healthy = false;
        for (let attempt = 0; attempt < 150; attempt++) {
          try { if ((await fetch(`http://${listen}/ready`)).status === 200) { healthy = true; break; } } catch {}
          if (run("docker", ["inspect", "--format", "{{.State.Running}}", imageContainer]) !== "true") break;
          await sleep(100);
        }
        assert.equal(healthy, true, run("docker", ["logs", imageContainer]));
        assert.equal((await call("/read", { query: { read: "Item", select: ["id", "checked"] } })).rows[0].checked, true);
        run("docker", ["exec", imageContainer, "node", "tools/app-deploy.mjs", "ready", "--config", "/run/aip/service.json"]);
        run("docker", ["stop", "--time", "15", imageContainer]);
        assert.equal(run("docker", ["inspect", "--format", "{{.State.ExitCode}}", imageContainer]), "0");
        run("docker", ["rm", imageContainer]); imageContainer = undefined;
      }
      assert.equal(sql("SELECT checked FROM aip_deploy_live.item WHERE id=11;"), "t");
      assert.deepEqual(JSON.parse(await readFile(join(operator, "jwks.json"))), { keys: [jwk] });
      console.log("Linux app image: build, non-root/read-only/cap-drop, ready, authenticated read, SIGTERM, recreate, external DB/config preservation PASS");
    } else console.log("Docker image lifecycle NOT RUN: requires Linux and AIP_TEST_APP_DOCKER=1");
    sql(backup);
    contract = fingerprint(bundles[0]); await start(bundles[0]);
    assert.equal((await call("/read", { query: { read: "Item", select: ["id", "checked"] } })).rows[0].checked, true);
    assert.equal((await call("/apply", { request: { apply: "Item.mark", target: { ids: ["11"] } }, key: "deploy-write-1" })).replayed, true);
    await stop();
    console.log("old definition rejected after migration; pre-upgrade PostgreSQL dump restored; old package read/idempotency PASS");
    if (process.platform === "linux" && process.env.AIP_TEST_APP_WORKERS === "1") {
      for (const language of ["node", "python"]) {
        const input = join(base, `worker-input-${language}`); const bundle = join(base, `worker-bundle-${language}`);
        await mkdir(join(input, "extensions"), { recursive: true });
        const extension = language === "node" ? "mjs" : "py";
        await writeFile(join(input, "app.aip"), await readFile(join(repo, "prototype/example/app.aip")));
        await writeFile(join(input, `extensions/event.${extension}`), await readFile(join(repo, `prototype/example/extensions/event.${extension}`)));
        await writeFile(join(input, "aip.app.json"), JSON.stringify({ name: `worker-${language}`, version: "1.0.0", definition: "app.aip", extensions: { directory: "extensions", language, entrypoints: [`event.${extension}`], enable_writes: true } }));
        run(process.execPath, [join(repo, "product/tools/package.mjs"), "--manifest", input, "--out", bundle]);
        const installed = join(base, `worker-installed-${language}`);
        run(process.execPath, [join(bundle, "tools/app-deploy.mjs"), "install", "--out", installed], { cwd: base });
        await rm(input, { recursive: true }); await rm(bundle, { recursive: true });
        const report = JSON.parse(run(process.execPath, [join(repo, "product/tools/smoke.mjs"), "--bin", join(installed, "bin/aip"), "--sdk-package", join(installed, "sdk/aip-sdk-0.1.0.tgz"), "--app-definition", join(installed, "app/app.aip"), "--workers", language, "--extension-dir", join(installed, "app/extensions")], { cwd: base, env: { ...env, AIP_TEST_DATABASE_URL: env.AIP_DEPLOY_TEST_DSN } }));
        assert.equal(report.ok, true); assert.equal(report.worker, language);
        for (const wire of report.wires) { assert.equal(wire.extension.writesApplied, 1); assert.equal(wire.operation.job.status, "SUCCEEDED"); }
        console.log(`${language} installed package: both Id wires, installed SDK, authenticated HTTP read/apply, isolated extension and durable job PASS`);
      }
    } else console.log("Packaged worker execution NOT RUN: requires Linux and AIP_TEST_APP_WORKERS=1");
  } finally {
    try { await stop(); } finally {
      try {
        if (imageContainer) run("docker", ["rm", "--force", imageContainer]);
        if (owned) run("docker", ["rm", "--force", container]);
      } finally {
        if (image) spawnSync("docker", ["image", "rm", image], { stdio: "ignore" });
        await rm(base, { recursive: true, force: true });
      }
    }
  }
});

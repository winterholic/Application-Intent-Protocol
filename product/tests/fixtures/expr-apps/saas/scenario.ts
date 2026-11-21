import assert from "node:assert/strict";
import { connect } from "@aip/sdk";
import { contract } from "./binding.js";
import { baseUrl, scenario, sql, schema, tokens } from "./check.js";

const as = (who: string) => connect(baseUrl, tokens[who]!, contract);
const alice = as("alice"); // Acme OWNER
const bob = as("bob"); // Acme MEMBER
const vera = as("vera"); // Acme VIEWER
const carol = as("carol"); // Globex ADMIN
const dave = as("dave"); // Globex MEMBER

// 시나리오 사이에 넘기는 상태
const s: { project?: string; task?: string; tasks: string[] } = { tasks: [] };
const GLOBEX_PROJECT = "1";
const GLOBEX_TASK = "1";

function ok<T extends { ok: boolean }>(result: T, what: string): asserts result is T & { ok: true } {
  assert.equal(result.ok, true, `${what}: ${JSON.stringify(result)}`);
}
function denied(result: { ok: boolean; code?: string }, code: string, what: string) {
  assert.equal(result.ok, false, `${what}: 거부돼야 함 ${JSON.stringify(result)}`);
  assert.equal(result.code, code, `${what}: ${JSON.stringify(result)}`);
}

await scenario("session-actor-binding", async () => {
  assert.deepEqual(await alice.session(), { actorId: "1" });
  assert.deepEqual(await carol.session(), { actorId: "4" });
});

await scenario("tenant-read-isolation", async () => {
  const orgs = await alice.read({ read: "Org", select: ["id", "name"] });
  assert.deepEqual(orgs.rows.map((r) => r.name), ["Acme"]);
  const users = await alice.read({ read: "User", select: ["id", "name"], sort: [{ field: "id", dir: "asc" }] });
  assert.deepEqual(users.rows.map((r) => r.name), ["Alice", "Bob", "Vera"], "Globex 사용자는 안 보인다");
  const tasks = await alice.read({ read: "Task", select: ["id", "title"] });
  assert.equal(tasks.rows.length, 0, "Globex 할 일은 Acme에게 0건");
  const theirs = await carol.read({ read: "Task", select: ["id", "title"] });
  assert.deepEqual(theirs.rows.map((r) => r.title), ["globex-secret-task"]);
});

await scenario("field-policy-email", async () => {
  const forBob = await bob.read({ read: "User", select: ["id", "name", "email"], sort: [{ field: "id", dir: "asc" }] });
  assert.deepEqual(forBob.rows.map((r) => r.email), [null, "bob@acme.test", null], "MEMBER는 본인 이메일만");
  const forAlice = await alice.read({ read: "User", select: ["id", "email"], sort: [{ field: "id", dir: "asc" }] });
  assert.deepEqual(forAlice.rows.map((r) => r.email), ["alice@acme.test", "bob@acme.test", "vera@acme.test"], "OWNER는 전부");
});

await scenario("admin-creates-project-member-cannot", async () => {
  const made = await alice.bundle({ atomic: [{ create: "Project", values: { org: "1", name: "Launch", status: "ACTIVE", owner: "1", version: 0 } }] }, { key: "saas-proj-1" });
  ok(made, "project create");
  s.project = made.created[0]!.id;
  const nope = await bob.bundle({ atomic: [{ create: "Project", values: { org: "1", name: "Rogue", status: "ACTIVE", owner: "2", version: 0 } }] }, { key: "saas-proj-bob" });
  denied(nope, "WIRE.FORBIDDEN", "MEMBER의 프로젝트 생성");
  assert.equal(sql(`SELECT count(*) FROM ${schema}.project WHERE name='Rogue'`), "0");
});

await scenario("member-creates-task-with-relations", async () => {
  const made = await bob.bundle({ atomic: [{ create: "Task", values: { org: "1", project: s.project!, title: "write spec", description: "first draft", assignee: "2", creator: "2", status: "TODO", priority: 3, version: 0 } }] }, { key: "saas-task-1" });
  ok(made, "task create");
  s.task = made.created[0]!.id;
  const page = await bob.read({ read: "Task", select: ["id", "title", "status", "version", "commentCount", { project: { select: ["id", "name"] } }, { assignee: { select: ["id", "name"] } }], filter: [{ field: "id", op: "eq", value: s.task! }] });
  const row = page.rows[0] as unknown as Record<string, unknown>;
  assert.equal(row.title, "write spec");
  assert.deepEqual(row.project, { id: s.project, name: "Launch" });
  assert.deepEqual(row.assignee, { id: "2", name: "Bob" });
  assert.equal(row.commentCount, 0);
});

await scenario("partial-update-keeps-omitted-fields", async () => {
  const r = await bob.apply({ apply: "Task.edit", target: { ids: [s.task!] }, values: { title: "write spec v2" }, expect: { version: 0 } }, { key: "saas-edit-1" });
  ok(r, "partial edit");
  const row = (await bob.read({ read: "Task", select: ["id", "title", "description", "assignee", "priority", "version"], filter: [{ field: "id", op: "eq", value: s.task! }] })).rows[0] as unknown as Record<string, unknown>;
  assert.equal(row.title, "write spec v2");
  assert.equal(row.description, "first draft", "생략한 description 유지");
  assert.equal(row.assignee, "2", "생략한 assignee 유지");
  assert.equal(row.priority, 3);
  assert.equal(row.version, 1, "to version = version + 1");
  assert.equal(sql(`SELECT title||'|'||description||'|'||version FROM ${schema}.task WHERE id=${s.task}`), "write spec v2|first draft|1");
});

await scenario("optimistic-lock-conflict-then-retry", async () => {
  // alice가 version 0 시점에 읽었다고 가정하고 늦게 보낸다 -> 충돌
  const stale = await alice.apply({ apply: "Task.edit", target: { ids: [s.task!] }, values: { priority: 5 }, expect: { version: 0 } }, { key: "saas-edit-stale" });
  denied(stale, "WIRE.CONFLICT", "낡은 version");
  assert.equal(sql(`SELECT priority||'|'||version FROM ${schema}.task WHERE id=${s.task}`), "3|1", "충돌한 요청은 아무것도 바꾸지 않는다");
  const retry = await alice.apply({ apply: "Task.edit", target: { ids: [s.task!] }, values: { priority: 5, assignee: null }, expect: { version: 1 } }, { key: "saas-edit-fresh" });
  ok(retry, "새 version으로 재시도");
  assert.equal(sql(`SELECT priority||'|'||coalesce(assignee_id::text,'null')||'|'||version FROM ${schema}.task WHERE id=${s.task}`), "5|null|2", "null로 nullable Ref 해제");
});

await scenario("idempotent-replay-single-effect", async () => {
  const req = { apply: "Task.edit" as const, target: { ids: [s.task!] }, values: { description: "replayed" }, expect: { version: 2 } };
  const a = await alice.apply(req, { key: "saas-edit-replay" });
  const b = await alice.apply(req, { key: "saas-edit-replay" });
  ok(a, "first");
  assert.equal((b as { replayed?: boolean }).replayed, true, "같은 key 재전송은 replay");
  assert.equal(sql(`SELECT version FROM ${schema}.task WHERE id=${s.task}`), "3", "version은 한 번만 증가");
});

await scenario("state-transitions-and-role-guards", async () => {
  // assignee가 비어 있으니 owner(admin)만 시작할 수 있고, VIEWER와 무관한 MEMBER는 막힌다.
  denied(await bob.apply({ apply: "Task.start", target: { ids: [s.task!] } }, { key: "saas-start-bob" }), "WIRE.FORBIDDEN", "assignee 아닌 MEMBER");
  denied(await vera.apply({ apply: "Task.start", target: { ids: [s.task!] } }, { key: "saas-start-vera" }), "WIRE.FORBIDDEN", "VIEWER");
  ok(await alice.apply({ apply: "Task.start", target: { ids: [s.task!] } }, { key: "saas-start-alice" }), "admin start");
  // 순서를 건너뛴 전이(TODO를 건너뛰고 start 재호출)는 from 조건으로 막힌다
  const again = await alice.apply({ apply: "Task.start", target: { ids: [s.task!] } }, { key: "saas-start-again" });
  assert.equal(again.ok, false, `from status = TODO 위반: ${JSON.stringify(again)}`);
  ok(await alice.apply({ apply: "Task.complete", target: { ids: [s.task!] } }, { key: "saas-complete" }), "complete");
  const afterDone = await alice.apply({ apply: "Task.edit", target: { ids: [s.task!] }, values: { title: "too late" }, expect: { version: 5 } }, { key: "saas-edit-done" });
  assert.equal(afterDone.ok, false, `DONE 뒤 edit 금지: ${JSON.stringify(afterDone)}`);
  assert.equal(sql(`SELECT status FROM ${schema}.task WHERE id=${s.task}`), "DONE");
});

await scenario("role-negatives", async () => {
  const input = (who: string) => ({ create: "Task" as const, values: { org: "1", project: s.project!, title: `by ${who}`, assignee: null, creator: who === "vera" ? "3" : "2", status: "TODO" as const, priority: 1, version: 0 } });
  denied(await vera.bundle({ atomic: [input("vera")] }, { key: "saas-vera-create" }), "WIRE.FORBIDDEN", "VIEWER의 할 일 생성");
  // 다른 사람이 만든 할 일을 MEMBER가 수정: creator도 assignee도 아님
  const other = await alice.bundle({ atomic: [{ create: "Task", values: { org: "1", project: s.project!, title: "alice-task", assignee: null, creator: "1", status: "TODO", priority: 2, version: 0 } }] }, { key: "saas-alice-task" });
  ok(other, "alice task");
  s.tasks.push(other.created[0]!.id);
  denied(await bob.apply({ apply: "Task.edit", target: { ids: [s.tasks[0]!] }, values: { title: "hijack" }, expect: { version: 0 } }, { key: "saas-bob-hijack" }), "WIRE.FORBIDDEN", "MEMBER가 남의 할 일 수정");
  denied(await bob.apply({ apply: "Project.archive", target: { ids: [s.project!] } }, { key: "saas-bob-archive" }), "WIRE.FORBIDDEN", "MEMBER의 프로젝트 보관");
  denied(await bob.bundle({ atomic: [{ create: "User", values: { org: "1", name: "Eve", email: "eve@acme.test", role: "MEMBER" } }] }, { key: "saas-bob-invite" }), "WIRE.FORBIDDEN", "MEMBER의 초대");
  denied(await alice.bundle({ atomic: [{ create: "User", values: { org: "1", name: "Mallory", email: "m@acme.test", role: "OWNER" } }] }, { key: "saas-alice-owner" }), "WIRE.FORBIDDEN", "OWNER 역할 부여 금지");
  ok(await alice.bundle({ atomic: [{ create: "User", values: { org: "1", name: "Frank", email: "frank@acme.test", role: "MEMBER" } }] }, { key: "saas-alice-invite" }), "admin 초대");
  assert.equal(sql(`SELECT count(*) FROM ${schema}."user" WHERE name IN ('Eve','Mallory')`), "0");
  assert.equal(sql(`SELECT title FROM ${schema}.task WHERE id=${s.tasks[0]}`), "alice-task");
});

await scenario("cross-tenant-negatives", async () => {
  denied(await carol.apply({ apply: "Task.edit", target: { ids: [s.tasks[0]!] }, values: { title: "pwn" }, expect: { version: 0 } }, { key: "saas-x-edit" }), "WIRE.MISSING_TARGET", "다른 테넌트 행 수정은 존재 자체를 숨긴다");
  denied(await alice.apply({ apply: "Task.edit", target: { ids: [GLOBEX_TASK] }, values: { title: "pwn" }, expect: { version: 0 } }, { key: "saas-x-edit2" }), "WIRE.MISSING_TARGET", "Acme가 Globex 행 수정");
  denied(await carol.apply({ apply: "Project.archive", target: { ids: [s.project!] } }, { key: "saas-x-archive" }), "WIRE.MISSING_TARGET", "다른 테넌트 프로젝트 보관");
  const hidden = await carol.read({ read: "Task", select: ["id"], filter: [{ field: "id", op: "eq", value: s.tasks[0]! }] });
  assert.equal(hidden.rows.length, 0, "id를 알아도 다른 테넌트 행은 읽히지 않는다");
  // 다른 테넌트의 프로젝트에 할 일 만들기(참조 대상이 안 보임)
  denied(await bob.bundle({ atomic: [{ create: "Task", values: { org: "1", project: GLOBEX_PROJECT, title: "into globex", assignee: null, creator: "2", status: "TODO", priority: 1, version: 0 } }] }, { key: "saas-x-create-1" }), "WIRE.MISSING_TARGET", "Globex 프로젝트로 생성");
  // org를 남의 테넌트로 위조
  denied(await carol.bundle({ atomic: [{ create: "Task", values: { org: "1", project: GLOBEX_PROJECT, title: "forged org", assignee: null, creator: "4", status: "TODO", priority: 1, version: 0 } }] }, { key: "saas-x-create-2" }), "WIRE.MISSING_TARGET", "org 위조: 안 보이는 조직은 없는 조직과 같다");
  // 담당자를 다른 테넌트 사용자로 지정
  denied(await alice.apply({ apply: "Task.edit", target: { ids: [s.tasks[0]!] }, values: { assignee: "5" }, expect: { version: 0 } }, { key: "saas-x-assign" }), "WIRE.MISSING_TARGET", "남의 테넌트 사용자를 담당자로");
  // 남의 할 일에 댓글
  denied(await carol.bundle({ atomic: [{ create: "Comment", values: { org: "2", task: s.tasks[0]!, author: "4", body: "hi" } }] }, { key: "saas-x-comment" }), "WIRE.MISSING_TARGET", "남의 할 일에 댓글");
  assert.equal(sql(`SELECT count(*) FROM ${schema}.task WHERE title IN ('pwn','into globex','forged org')`), "0", "DB에는 아무것도 남지 않음");
  assert.equal(sql(`SELECT title||'|'||version FROM ${schema}.task WHERE id=${GLOBEX_TASK}`), "globex-secret-task|0");
});

await scenario("one-to-many-traverse-and-aggregate-count", async () => {
  ok(await bob.bundle({ atomic: [
    { create: "Comment", values: { org: "1", task: s.tasks[0]!, author: "2", body: "first" } },
    { create: "Comment", values: { org: "1", task: s.tasks[0]!, author: "2", body: "second" } },
  ] }, { key: "saas-comments" }), "comments");
  const row = (await alice.read({ read: "Task", select: ["id", "commentCount", { comments: { select: ["id", "body", "author"], limit: 10 } }], filter: [{ field: "id", op: "eq", value: s.tasks[0]! }] })).rows[0] as unknown as { commentCount: number; comments: { body: string }[] };
  assert.equal(row.commentCount, 2);
  assert.deepEqual(row.comments.map((c) => c.body), ["first", "second"]);
  const other = await carol.read({ read: "Comment", select: ["id"] });
  assert.equal(other.rows.length, 0, "Globex는 Acme 댓글이 안 보인다");
});

await scenario("cursor-pagination-with-filter-and-sort", async () => {
  const ops = Array.from({ length: 7 }, (_, i) => ({ create: "Task" as const, values: { org: "1", project: s.project!, title: `bulk-${i}`, assignee: null, creator: "1", status: "TODO" as const, priority: (i % 5) + 1, version: 0 } }));
  // 정적 배열이 아니면 bundle이 요구하는 비어 있지 않은 튜플 타입으로 직접 좁혀야 한다.
  const bulk = ops as [(typeof ops)[number], ...(typeof ops)[number][]];
  ok(await alice.bundle({ atomic: bulk }, { key: "saas-bulk" }), "bulk create");
  const seen: string[] = [];
  let after: { id: string } | undefined;
  for (let guard = 0; guard < 10; guard++) {
    const page = await alice.read({ read: "Task", select: ["id", "title"], filter: [{ field: "project", op: "eq", value: s.project! }], sort: [{ field: "id", dir: "desc" }], limit: 3, ...(after ? { after } : {}) });
    const rows = page.rows as unknown as { id: string; title: string }[];
    if (rows.length === 0) break;
    seen.push(...rows.map((r) => r.id));
    after = { id: rows[rows.length - 1]!.id };
    if (rows.length < 3) break;
  }
  const expected = sql(`SELECT string_agg(id::text, ',' ORDER BY id DESC) FROM ${schema}.task WHERE project_id=${s.project}`).split(",");
  assert.deepEqual(seen, expected, "cursor로 이어 읽은 전체가 DB의 id 내림차순과 같다");
  assert.ok(seen.length >= 9);
  const high = await alice.read({ read: "Task", select: ["id", "priority"], filter: [{ field: "priority", op: "gte", value: 4 }, { field: "project", op: "eq", value: s.project! }], sort: [{ field: "priority", dir: "desc" }], limit: 20 });
  const pri = (high.rows as unknown as { priority: number }[]).map((r) => r.priority);
  assert.ok(pri.length > 0 && pri.every((p) => p >= 4) && pri.every((p, i) => i === 0 || pri[i - 1]! >= p));
});

await scenario("archive-cascade-and-state-guard", async () => {
  ok(await alice.apply({ apply: "Project.rename", target: { ids: [s.project!] }, values: { name: "Launch 2.0" }, expect: { version: 0 } }, { key: "saas-rename" }), "rename");
  denied(await alice.apply({ apply: "Project.rename", target: { ids: [s.project!] }, values: { name: "stale" }, expect: { version: 0 } }, { key: "saas-rename-stale" }), "WIRE.CONFLICT", "프로젝트 낡은 version");
  ok(await alice.apply({ apply: "Project.archive", target: { ids: [s.project!] } }, { key: "saas-archive" }), "archive");
  assert.equal(sql(`SELECT count(*) FROM ${schema}.task WHERE project_id=${s.project} AND status<>'DONE'`), "0", "선언 효과 update many가 같은 트랜잭션에서 할 일을 DONE으로");
  const again = await alice.apply({ apply: "Project.rename", target: { ids: [s.project!] }, values: { name: "after archive" }, expect: { version: 2 } }, { key: "saas-rename-archived" });
  assert.equal(again.ok, false, `ARCHIVED 프로젝트 수정 금지: ${JSON.stringify(again)}`);
});

await scenario("delete-transition-admin-only", async () => {
  const victim = s.tasks[0]!;
  denied(await bob.apply({ apply: "Task.remove", target: { ids: [victim] } }, { key: "saas-rm-bob" }), "WIRE.FORBIDDEN", "MEMBER의 삭제");
  denied(await dave.apply({ apply: "Task.remove", target: { ids: [victim] } }, { key: "saas-rm-dave" }), "WIRE.MISSING_TARGET", "다른 테넌트의 삭제");
  ok(await alice.apply({ apply: "Task.remove", target: { ids: [victim] } }, { key: "saas-rm-alice" }), "admin 삭제");
  assert.equal(sql(`SELECT count(*) FROM ${schema}.task WHERE id=${victim}`), "0");
  assert.equal(sql(`SELECT count(*) FROM ${schema}.comment WHERE task_id=${victim}`), "0", "댓글 cascade 삭제");
});

await scenario("stored-tenant-columns-consistent", async () => {
  assert.ok(Number(sql(`SELECT count(*) FROM ${schema}.task t JOIN ${schema}.project p ON p.id=t.project_id`)) >= 9, "비교 대상이 실제로 있다");
  assert.equal(sql(`SELECT count(*) FROM ${schema}.task t JOIN ${schema}.project p ON p.id=t.project_id WHERE t.org_id<>p.org_id`), "0", "할 일과 프로젝트의 org가 항상 같다");
  assert.equal(sql(`SELECT count(*) FROM ${schema}.comment c JOIN ${schema}.task t ON t.id=c.task_id WHERE c.org_id<>t.org_id`), "0");
  assert.equal(sql(`SELECT count(*) FROM ${schema}.comment`), "0", "삭제된 할 일의 댓글은 선언 효과 delete many로 함께 지워졌다");
});

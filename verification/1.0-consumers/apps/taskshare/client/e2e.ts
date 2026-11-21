import { connect } from "@aip/sdk";
import { contract, contractFingerprint } from "./binding.ts";
import assert from "node:assert/strict";

const URL_ = process.env.AIP_URL ?? "http://127.0.0.1:18081";
const run = process.env.RUN_ID ?? String(Date.now());
const as = (t: string) => connect(URL_, t, contract);
const alice = as(process.env.TOKEN_ALICE!), bob = as(process.env.TOKEN_BOB!), carol = as(process.env.TOKEN_CAROL!);
const anon = connect(URL_, null, contract);
let n = 0;
const step = (name: string, extra?: unknown) => console.log(`PASS ${String(++n).padStart(2, "0")} ${name}${extra === undefined ? "" : " " + JSON.stringify(extra)}`);

// 인증 조회
const who = await alice.session();
assert.equal(who.actorId, "1"); step("session alice", who);
const me = await alice.read({ read: "Member", select: ["id", "name", "email"] } as const);
assert.equal(me.rows.length, 3);
const mine = me.rows.find((r: any) => r.id === "1")!, other = me.rows.find((r: any) => r.id === "2")!;
assert.equal(mine.email, "alice@example.test"); assert.equal(other.email, null); step("member directory: own email visible, others' email hidden by field policy", me.rows);

// 생성
const made = await alice.bundle({ atomic: [{ create: "TodoList", values: { owner: "1", title: `Groceries ${run}`, archived: false } }] }, { key: `list-${run}` });
assert.ok(made.ok); const listId = made.created[0].id!; step("create list", { listId });
const item = await alice.bundle({ atomic: [
  { create: "TodoItem", values: { list: listId, createdBy: "1", title: "milk", notes: null, priority: "HIGH", dueAt: "2026-12-01T00:00:00Z", done: false, version: 0, assignee: null } },
  { create: "TodoItem", values: { list: listId, createdBy: "1", title: "eggs", notes: "free range", priority: "LOW", dueAt: null, done: false, version: 0, assignee: null } },
] }, { key: `items-${run}` });
assert.ok(item.ok); const [milk, eggs] = item.created.map((c) => c.id!); step("create 2 items atomically", { milk, eggs });

// 수정(patch+expect), 낙관적 잠금 충돌, 완료, 멱등
const edit = await alice.apply({ apply: "TodoItem.edit", target: { ids: [milk] }, values: { title: "milk 2L" }, expect: { version: 0 } } as const, { key: `edit-${run}` });
assert.ok(edit.ok); step("patch edit with expect version", edit);
const conflict = await alice.apply({ apply: "TodoItem.edit", target: { ids: [milk] }, values: { title: "stale" }, expect: { version: 0 } } as const, { key: `edit-stale-${run}` });
assert.equal(conflict.ok, false); assert.equal((conflict as any).code, "WIRE.CONFLICT"); step("stale expect -> WIRE.CONFLICT", { code: (conflict as any).code });
const done1 = await alice.apply({ apply: "TodoItem.complete", target: { ids: [eggs] } } as const, { key: `done-${run}` });
const done2 = await alice.apply({ apply: "TodoItem.complete", target: { ids: [eggs] } } as const, { key: `done-${run}` });
assert.ok(done1.ok && done2.ok && (done2 as any).replayed === true); step("complete + same key replayed", { replayed: (done2 as any).replayed });

// 관계·조건 조회
const lists = await alice.read({ read: "TodoList", select: ["id", "title", "openCount", { owner: { select: ["id", "name"] } }, { TodoItem: { select: ["id", "title", "done"], limit: 10 } }], filter: [{ field: "archived", op: "eq", value: false }], sort: [{ field: "id", dir: "asc" }] } as any);
console.log(JSON.stringify(lists.rows));
step("relation read (owner + items + openCount)", lists.rows.length);
const open = await alice.read({ read: "TodoItem", select: ["id", "title", "dueAt", "version"] as const, filter: [{ field: "list", op: "eq", value: listId }, { field: "done", op: "eq", value: false }, { field: "title", op: "contains", value: "milk" }], sort: [{ field: "dueAt", dir: "asc" }] } as any);
assert.equal(open.rows.length, 1); step("filter+sort read", open.rows);

// 권한 없는 요청
assert.equal((await carol.read({ read: "TodoList", select: ["id", "title"] } as const)).rows.length, 0); step("carol sees no lists");
const denied = await carol.apply({ apply: "TodoItem.complete", target: { ids: [milk] } } as const, { key: `carol-${run}` });
assert.equal(denied.ok, false); step("carol complete -> denied", { code: (denied as any).code });
assert.equal((await anon.read({ read: "TodoList", select: ["id"] } as const)).rows.length, 0); step("anonymous read -> no rows");
let fieldErr = ""; try { await alice.read({ read: "Member", select: ["id", "secret"] } as any); } catch (e: any) { fieldErr = e.code ?? e.message; }
assert.match(fieldErr, /FIELD_NOT_EXPOSED|BAD|NOT_EXPOSED/); step("unexposed field -> rejected", fieldErr);

// 공유: viewer는 읽기만
const share = await alice.bundle({ atomic: [{ create: "ListShare", values: { list: listId, member: "2", role: "VIEWER" } }] }, { key: `share-${run}` });
if (!share.ok) console.log("SHARE FAIL", JSON.stringify(share)); assert.ok(share.ok); const shareId = share.created[0].id!; step("share list to bob VIEWER", { shareId });
assert.equal((await bob.read({ read: "TodoList", select: ["id"], filter: [{ field: "title", op: "prefix", value: `Groceries ${run}` }] } as const)).rows.length, 1); step("bob reads shared list");
const bobComplete = await bob.apply({ apply: "TodoItem.complete", target: { ids: [milk] } } as const, { key: `bob-${run}` });
assert.equal(bobComplete.ok, false); step("viewer complete -> denied", { code: (bobComplete as any).code });
const bobCreate = await bob.bundle({ atomic: [{ create: "TodoItem", values: { list: listId, createdBy: "2", title: "hack", notes: null, priority: "LOW", dueAt: null, done: false, version: 0, assignee: null } }] }, { key: `bobc-${run}` });
assert.equal(bobCreate.ok, false); step("viewer create item -> denied", { code: (bobCreate as any).code });

// 확장
const re = await alice.writeExtension("TodoItem.reschedule", { id: milk, rule: "WEEKDAYS" }, { key: `resched-${run}` });
console.log(JSON.stringify(re));
assert.ok((re as any).ok); step("node write extension reschedule", re);
const after = await alice.read({ read: "TodoItem", select: ["id", "dueAt", "version"] as const, filter: [{ field: "id", op: "eq", value: milk }] } as any);
console.log(JSON.stringify(after.rows));
step("extension result persisted", after.rows);

// 삭제
const rm = await alice.apply({ apply: "TodoItem.remove", target: { ids: [eggs] } } as const, { key: `rm-${run}` });
assert.ok(rm.ok); step("delete item (transition delete)", rm);
const unshare = await alice.apply({ apply: "ListShare.unshare", target: { ids: [shareId] } } as const, { key: `unshare-${run}` });
assert.ok(unshare.ok); step("unshare", unshare);
assert.equal((await bob.read({ read: "TodoList", select: ["id"], filter: [{ field: "title", op: "prefix", value: `Groceries ${run}` }] } as const)).rows.length, 0); step("bob lost access");
console.log("ALL PASS");

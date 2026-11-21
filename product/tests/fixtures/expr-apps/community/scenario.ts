import assert from "node:assert/strict";
import { connect } from "@aip/sdk";
import { contract, type Id } from "./binding.js";
import { baseUrl, scenario, sql, schema, tokens } from "./check.js";

const mia = connect(baseUrl, tokens.mia!, contract); // 작성자
const noah = connect(baseUrl, tokens.noah!, contract); // 독자
const mod = connect(baseUrl, tokens.mod!, contract); // 운영자
// SDK 읽기 cache(maxAgeMs 1s)는 같은 client의 쓰기로만 무효화된다. 다른 사용자의 변경을 바로 보려면 새 client가 필요하다.
const anon = () => connect(baseUrl, null, contract); // 로그인하지 않은 방문자

function ok<T extends { ok: boolean }>(result: T, what: string): asserts result is T & { ok: true } {
  assert.equal(result.ok, true, `${what}: ${JSON.stringify(result)}`);
}
function denied(result: { ok: boolean; code?: string }, code: string, what: string) {
  assert.equal(result.ok, false, `${what}: 거부돼야 함 ${JSON.stringify(result)}`);
  assert.equal(result.code, code, `${what}: ${JSON.stringify(result)}`);
}
const draft = (board: string, title: string, author = "1") =>
  ({ create: "Post" as const, values: { board, author, title, body: `body of ${title}`, status: "DRAFT" as const, views: 0, version: 0 } });

const posts: string[] = []; // 생성 순서대로의 post id (일반 12개: board1 8개, board2 4개)

await scenario("draft-visibility-author-only", async () => {
  const made = await mia.bundle({ atomic: [
    draft("1", "alpha one"), draft("1", "alpha two"), draft("1", "beta three"), draft("1", "beta four"),
    draft("1", "gamma five"), draft("1", "gamma six"), draft("1", "delta seven"), draft("1", "delta eight"),
    draft("2", "tech nine"), draft("2", "tech ten"), draft("2", "tech eleven"), draft("2", "tech twelve"),
  ] }, { key: "comm-create-12" });
  ok(made, "12 drafts");
  posts.push(...made.created.map((c) => c.id));
  assert.equal(posts.length, 12);
  const mine = await mia.read({ read: "Post", select: ["id", "status", "publishedAt"], limit: 20 });
  assert.equal(mine.rows.length, 12, "작성자는 자기 초안을 본다");
  assert.ok(mine.rows.every((r) => r.status === "DRAFT" && r.publishedAt === null));
  assert.equal((await noah.read({ read: "Post", select: ["id"], limit: 20 })).rows.length, 0, "다른 독자에게 초안은 보이지 않는다");
  assert.equal((await anon().read({ read: "Post", select: ["id"], limit: 20 })).rows.length, 0, "익명에게도");
  const modSees = await mod.read({ read: "Post", select: ["id"], limit: 20 });
  assert.equal(modSees.rows.length, 12, "운영자는 전부 본다");
  denied(await noah.apply({ apply: "Post.publish", target: { ids: [posts[0]!] } }, { key: "comm-noah-publish-draft" }), "WIRE.MISSING_TARGET", "남의 초안 게시는 존재를 숨긴다");
});

await scenario("draft-to-published-bulk-transition", async () => {
  const ten = posts.slice(0, 10);
  const r = await mia.apply({ apply: "Post.publish", target: { ids: ten } }, { key: "comm-publish-10" });
  ok(r, "bulk publish");
  assert.equal(sql(`SELECT count(*) FROM ${schema}.post WHERE status='PUBLISHED' AND published_at IS NOT NULL`), "10", "to publishedAt = now가 함께 기록");
  assert.equal(sql(`SELECT count(*) FROM ${schema}.post WHERE status='DRAFT'`), "2");
  // 이미 게시된 글을 다시 게시: from 조건으로 거부, 전체가 롤백(남은 draft는 안 바뀐다)
  const again = await mia.apply({ apply: "Post.publish", target: { ids: [posts[0]!, posts[10]!] } }, { key: "comm-publish-again" });
  assert.equal(again.ok, false, `중복 게시는 거부: ${JSON.stringify(again)}`);
  assert.equal(sql(`SELECT status FROM ${schema}.post WHERE id=${posts[10]}`), "DRAFT", "묶음 중 하나가 실패하면 전부 롤백");
  assert.equal((await noah.read({ read: "Post", select: ["id"], limit: 20 })).rows.length, 10, "독자는 게시된 10개만");
  assert.equal((await anon().read({ read: "Post", select: ["id"], limit: 20 })).rows.length, 10, "익명도 게시된 글은 읽는다");
  denied(await noah.apply({ apply: "Post.unpublish", target: { ids: [posts[0]!] } }, { key: "comm-noah-unpublish" }), "WIRE.FORBIDDEN", "독자의 게시 취소");
});

await scenario("moderation-hide-restore", async () => {
  const target = posts[2]!;
  denied(await noah.apply({ apply: "Post.hide", target: { ids: [target] } }, { key: "comm-noah-hide" }), "WIRE.FORBIDDEN", "독자의 숨김");
  denied(await mia.apply({ apply: "Post.hide", target: { ids: [target] } }, { key: "comm-mia-hide" }), "WIRE.FORBIDDEN", "작성자도 숨김 권한은 없다");
  ok(await mod.apply({ apply: "Post.hide", target: { ids: [target] } }, { key: "comm-mod-hide" }), "mod hide");
  assert.equal((await noah.read({ read: "Post", select: ["id"], filter: [{ field: "id", op: "eq", value: target }] })).rows.length, 0, "숨긴 글은 독자에게 사라진다");
  const own = await mia.read({ read: "Post", select: ["id", "status"], filter: [{ field: "id", op: "eq", value: target }] });
  assert.equal(own.rows[0]?.status, "HIDDEN", "작성자는 자기 글의 숨김 상태를 본다");
  const edit = await mia.apply({ apply: "Post.edit", target: { ids: [target] }, values: { title: "sneaky" }, expect: { version: 1 } }, { key: "comm-mia-edit-hidden" });
  assert.equal(edit.ok, false, `숨겨진 글 수정 금지: ${JSON.stringify(edit)}`);
  denied(await mod.apply({ apply: "Post.hide", target: { ids: [target] } }, { key: "comm-mod-hide-2" }), "WIRE.INVALID_STATE", "이미 숨긴 글");
  ok(await mod.apply({ apply: "Post.restore", target: { ids: [target] } }, { key: "comm-mod-restore" }), "restore");
  assert.equal(sql(`SELECT status||'|'||version FROM ${schema}.post WHERE id=${target}`), "PUBLISHED|3");
});

await scenario("edit-optimistic-lock-on-post", async () => {
  const p = posts[3]!;
  ok(await mia.apply({ apply: "Post.edit", target: { ids: [p] }, values: { title: "beta four (edited)" }, expect: { version: 1 } }, { key: "comm-edit-1" }), "edit");
  denied(await mia.apply({ apply: "Post.edit", target: { ids: [p] }, values: { body: "stale" }, expect: { version: 1 } }, { key: "comm-edit-stale" }), "WIRE.CONFLICT", "낡은 version");
  denied(await noah.apply({ apply: "Post.edit", target: { ids: [p] }, values: { title: "pwn" }, expect: { version: 2 } }, { key: "comm-noah-edit" }), "WIRE.FORBIDDEN", "독자의 수정");
  assert.equal(sql(`SELECT title||'|'||version FROM ${schema}.post WHERE id=${p}`), "beta four (edited)|2");
});

await scenario("views-counter-replay", async () => {
  const p = posts[4]!;
  for (const key of ["comm-view-a", "comm-view-b", "comm-view-c"]) ok(await noah.apply({ apply: "Post.view", target: { ids: [p] } }, { key }), key);
  const replay = await noah.apply({ apply: "Post.view", target: { ids: [p] } }, { key: "comm-view-c" });
  assert.equal((replay as { replayed?: boolean }).replayed, true);
  assert.equal(sql(`SELECT views FROM ${schema}.post WHERE id=${p}`), "3", "같은 key는 한 번만 증가");
  denied(await noah.apply({ apply: "Post.view", target: { ids: [posts[11]!] } }, { key: "comm-view-draft" }), "WIRE.MISSING_TARGET", "초안 조회수 증가는 불가");
});

await scenario("comments-relations-and-count", async () => {
  const p = posts[0]!;
  const made = await noah.bundle({ atomic: [
    { create: "Comment", values: { post: p, author: "2", body: "nice", hidden: false } },
    { create: "Comment", values: { post: p, author: "2", body: "agree", hidden: false } },
    { create: "Comment", values: { post: p, author: "2", body: "spam", hidden: false } },
  ] }, { key: "comm-comments" });
  ok(made, "comments");
  denied(await noah.bundle({ atomic: [{ create: "Comment", values: { post: posts[11]!, author: "2", body: "on draft", hidden: false } }] }, { key: "comm-comment-draft" }), "WIRE.MISSING_TARGET", "초안에 댓글");
  denied(await noah.bundle({ atomic: [{ create: "Comment", values: { post: p, author: "1", body: "as mia", hidden: false } }] }, { key: "comm-comment-forged" }), "WIRE.FORBIDDEN", "작성자 위조");
  const spam = made.created[2]!.id as Id<"Comment">; // created[]의 id는 resource별 타입이 아니라 합집합이라 직접 좁힌다
  denied(await mia.apply({ apply: "Comment.hide", target: { ids: [spam] } }, { key: "comm-hide-spam-mia" }), "WIRE.FORBIDDEN", "다른 사람 댓글을 일반 작성자가 숨김");
  ok(await mod.apply({ apply: "Comment.hide", target: { ids: [spam] } }, { key: "comm-hide-spam" }), "mod hides comment");
  const row = (await noah.read({ read: "Post", select: ["id", "title", "commentCount", { author: { select: ["id", "name"] } }, { board: { select: ["id", "name"] } }, { comments: { select: ["id", "body", "author"], limit: 10 } }], filter: [{ field: "id", op: "eq", value: p }] })).rows[0] as unknown as Record<string, unknown>;
  assert.equal(row.commentCount, 2, "숨긴 댓글은 where hidden = false로 센다");
  assert.deepEqual(row.author, { id: "1", name: "Mia" });
  assert.deepEqual(row.board, { id: "1", name: "General" });
  assert.deepEqual((row.comments as { body: string }[]).map((c) => c.body), ["nice", "agree"]);
});

await scenario("count-vs-sum-visibility-contract", async () => {
  const p = posts[1]!;
  ok(await mia.bundle({ atomic: [{ create: "Reaction", values: { post: p, member: "1", score: 5 } }] }, { key: "comm-react-mia" }), "mia");
  ok(await noah.bundle({ atomic: [{ create: "Reaction", values: { post: p, member: "2", score: 3 } }] }, { key: "comm-react-noah" }), "noah");
  denied(await noah.bundle({ atomic: [{ create: "Reaction", values: { post: p, member: "2", score: 4 } }] }, { key: "comm-react-dup" }), "WIRE.ALREADY_EXISTS", "unique(post, member)");
  const read = async (c: typeof mia) => (await c.read({ read: "Post", select: ["id", "reactionCount", "scoreSum"], filter: [{ field: "id", op: "eq", value: p }] })).rows[0] as unknown as { reactionCount: number; scoreSum: number | null };
  const forNoah = await read(noah);
  const forMod = await read(mod);
  console.log(`INFO count-vs-sum: 반응 2건(5점 mia, 3점 noah) -> noah 시점 count=${forNoah.reactionCount} sum=${forNoah.scoreSum}, 운영자 시점 count=${forMod.reactionCount} sum=${forMod.scoreSum}`);
  // count는 원본 rows read를 적용하지 않아 모두의 반응을 센다. sum은 호출자가 읽을 수 있는 행만 합산한다.
  assert.equal(forNoah.reactionCount, 2);
  assert.equal(forNoah.scoreSum, 3, `sum은 본인 반응만: ${JSON.stringify(forNoah)}`);
  assert.equal(forMod.reactionCount, 2);
  assert.ok(forMod.scoreSum === 0 || forMod.scoreSum === null, `운영자는 반응 행을 못 읽는다: ${JSON.stringify(forMod)}`);
});

await scenario("filter-sort-cursor-feed", async () => {
  // views를 다르게 만든다: posts[5]=2, posts[6]=2(동률), posts[7]=1, posts[8]=1
  for (const [i, n] of [[5, 2], [6, 2], [7, 1], [8, 1]] as const) {
    for (let k = 0; k < n; k++) ok(await noah.apply({ apply: "Post.view", target: { ids: [posts[i]!] } }, { key: `comm-feed-view-${i}-${k}` }), "view");
  }
  const board1 = await noah.read({ read: "Post", select: ["id", "title", "views"], filter: [{ field: "board", op: "eq", value: "1" }, { field: "views", op: "gte", value: 1 }], sort: [{ field: "views", dir: "desc" }], limit: 20 });
  const v = (board1.rows as unknown as { views: number }[]).map((r) => r.views);
  assert.ok(v.length === 4 && v.every((x, i) => i === 0 || v[i - 1]! >= x), `board 1에서 views>=1: ${v}`);
  const contains = await noah.read({ read: "Post", select: ["id", "title"], filter: [{ field: "title", op: "contains", value: "tech" }], limit: 20 });
  assert.deepEqual(contains.rows.map((r) => r.title).sort(), ["tech nine", "tech ten"], "tech 글 중 게시된 2개(eleven, twelve는 초안)");
  // cursor: 정렬 필드 views와 id 두 값으로 after를 만든다(서버가 다음 cursor를 주지 않는다)
  const seen: string[] = [];
  let after: { views: number; id: string } | undefined;
  for (let guard = 0; guard < 10; guard++) {
    const page = await noah.read({ read: "Post", select: ["id", "views"], sort: [{ field: "views", dir: "desc" }], limit: 3, ...(after ? { after } : {}) });
    const rows = page.rows as unknown as { id: string; views: number }[];
    if (rows.length === 0) break;
    seen.push(...rows.map((r) => r.id));
    const last = rows[rows.length - 1]!;
    after = { views: last.views, id: last.id };
    if (rows.length < 3) break;
  }
  const expected = sql(`SELECT string_agg(id::text, ',' ORDER BY views DESC, id ASC) FROM ${schema}.post WHERE status='PUBLISHED'`).split(",");
  assert.deepEqual(seen, expected, "cursor 쪽 이음이 DB의 (views desc, id asc 동률)와 같고 중복·누락이 없다");
  assert.equal(seen.length, 10);
});

await scenario("offset-pagination-on-comments", async () => {
  const p = posts[9]!;
  const many = Array.from({ length: 12 }, (_, i) => ({ create: "Comment" as const, values: { post: p, author: "2", body: `c${i}`, hidden: false } }));
  ok(await noah.bundle({ atomic: many as unknown as [(typeof many)[number], ...(typeof many)[number][]] }, { key: "comm-many-comments" }), "12 comments");
  const bodies: string[] = [];
  for (const offset of [0, 5, 10]) {
    const page = await noah.read({ read: "Comment", select: ["id", "body"], filter: [{ field: "post", op: "eq", value: p }], sort: [{ field: "id", dir: "asc" }], limit: 5, offset });
    bodies.push(...page.rows.map((r) => r.body));
  }
  assert.deepEqual(bodies, Array.from({ length: 12 }, (_, i) => `c${i}`), "offset 5씩 3쪽이 12개 전체");
  const past = await noah.read({ read: "Comment", select: ["id"], filter: [{ field: "post", op: "eq", value: p }], limit: 5, offset: 200 });
  assert.equal(past.rows.length, 0);
});

await scenario("budget-and-cursor-guard-negatives", async () => {
  await assert.rejects(noah.read({ read: "Post", select: ["id"], limit: 21 }), (e: { code?: string }) => typeof e.code === "string", "budget rows 20 초과");
  await assert.rejects(noah.read({ read: "Post", select: ["id"], sort: [{ field: "views", dir: "desc" }], after: { id: "1" } } as never), (e: { code?: string }) => e.code === "WIRE.BAD_CURSOR", "sort 필드가 빠진 after");
});

await scenario("board-published-count-aggregate", async () => {
  const boards = await anon().read({ read: "Board", select: ["id", "name", "publishedCount"], sort: [{ field: "id", dir: "asc" }] });
  const byId = Object.fromEntries((boards.rows as unknown as { id: string; publishedCount: number }[]).map((b) => [b.id, b.publishedCount]));
  const expected1 = Number(sql(`SELECT count(*) FROM ${schema}.post WHERE board_id=1 AND status='PUBLISHED'`));
  const expected2 = Number(sql(`SELECT count(*) FROM ${schema}.post WHERE board_id=2 AND status='PUBLISHED'`));
  assert.deepEqual([byId["1"], byId["2"]], [expected1, expected2]);
  assert.equal(expected1 + expected2, 10);
  ok(await mod.apply({ apply: "Post.hide", target: { ids: [posts[8]!] } }, { key: "comm-hide-count" }), "hide");
  const after = await anon().read({ read: "Board", select: ["id", "publishedCount"], sort: [{ field: "id", dir: "asc" }] });
  assert.equal((after.rows as unknown as { publishedCount: number }[]).reduce((n, b) => n + b.publishedCount, 0), 9, "숨기면 공개 count가 줄어든다");
});

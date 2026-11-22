// 여러 조직·역할·공유 권한 시나리오. 정의: teamdocs.aip. 시드: seed.sql(Ann은 Acme OWNER이자 Beta MEMBER).
import assert from "node:assert/strict";
import { connect } from "@aip/sdk";
import { contract } from "./binding.js";
import { baseUrl, scenario, sql, schema, tokens } from "./check.js";

const as = (who: string) => connect(baseUrl, tokens[who]!, contract);
// 읽기 cache가 다른 사용자의 변경을 가리지 않게 호출마다 새 client를 쓴다(consumer-apps.md F9).
const ann = () => as("ann"), ben = () => as("ben"), cat = () => as("cat"), dan = () => as("dan"), eve = () => as("eve"), fay = () => as("fay");
const ANN = "1", BEN = "2", CAT = "3", DAN = "4", EVE = "5", FAY = "6";
const ACME = "1", BETA = "2";
const BETA_DRAFT = "1", BETA_PUBLIC = "2";

function ok<T extends { ok: boolean }>(result: T, what: string): asserts result is T & { ok: true } {
  assert.equal(result.ok, true, `${what}: ${JSON.stringify(result)}`);
}
function denied(result: { ok: boolean; code?: string }, code: string, what: string) {
  assert.equal(result.ok, false, `${what}: 거부돼야 함 ${JSON.stringify(result)}`);
  assert.equal(result.code, code, `${what}: ${JSON.stringify(result)}`);
}
const titles = async (who: () => ReturnType<typeof as>) =>
  (await who().read({ read: "Doc", select: ["id", "title"], sort: [{ field: "id", dir: "asc" }] })).rows.map((r) => r.title);
const docId = (title: string) => sql(`SELECT id FROM ${schema}.doc WHERE title='${title}'`);
const create = (who: () => ReturnType<typeof as>, values: Record<string, unknown>, key: string) =>
  who().bundle({ atomic: [{ create: "Doc", values }] } as never, { key });

const s: { draft?: string; shared?: string; shareFay?: string } = {};

await scenario("D1-orgs-visible-by-membership-not-by-actor-column", async () => {
  const names = async (who: () => ReturnType<typeof as>) => (await who().read({ read: "Org", select: ["id", "name"], sort: [{ field: "id", dir: "asc" }] })).rows.map((r) => r.name);
  assert.deepEqual(await names(ann), ["Acme", "Beta"], "두 조직에 속한 Ann은 둘 다 본다");
  assert.deepEqual(await names(ben), ["Acme"]);
  assert.deepEqual(await names(eve), ["Beta"]);
  assert.deepEqual(await names(dan), ["Beta"]);
});

await scenario("D2-membership-admin-adds-member-only-owner-promotes", async () => {
  // Cat(ADMIN)이 Eve를 Acme MEMBER로 넣는다. ADMIN·OWNER 역할로 만드는 것은 allow가 거부한다.
  const added = await cat().bundle({ atomic: [{ create: "Membership", values: { org: ACME, user: EVE, role: "MEMBER" } }] }, { key: "td-m-add" });
  ok(added, "ADMIN이 MEMBER 추가");
  const asAdmin = await cat().bundle({ atomic: [{ create: "Membership", values: { org: ACME, user: DAN, role: "ADMIN" } }] }, { key: "td-m-admin" });
  denied(asAdmin, "WIRE.FORBIDDEN", "ADMIN이 ADMIN 추가");
  const byMember = await ben().bundle({ atomic: [{ create: "Membership", values: { org: ACME, user: DAN, role: "MEMBER" } }] }, { key: "td-m-member" });
  denied(byMember, "WIRE.FORBIDDEN", "MEMBER가 멤버 추가");
  const foreign = await cat().bundle({ atomic: [{ create: "Membership", values: { org: BETA, user: DAN, role: "MEMBER" } }] }, { key: "td-m-foreign" });
  assert.equal(foreign.ok, false, "다른 조직(Beta)의 멤버십은 Acme ADMIN이 만들 수 없다");
  assert.equal(sql(`SELECT count(*) FROM ${schema}.membership WHERE user_id=${DAN} AND org_id=1`), "0");
  const eveMembership = sql(`SELECT id FROM ${schema}.membership WHERE org_id=1 AND user_id=${EVE}`);
  const byAdmin = await cat().apply({ apply: "Membership.promote", target: { ids: [eveMembership] } } as never, { key: "td-m-promote-cat" });
  assert.equal(byAdmin.ok, false, "ADMIN은 승격하지 못한다(OWNER만)");
  const byOwner = await ann().apply({ apply: "Membership.promote", target: { ids: [eveMembership] } } as never, { key: "td-m-promote-ann" });
  ok(byOwner, "OWNER가 승격");
  assert.equal(sql(`SELECT role FROM ${schema}.membership WHERE id=${eveMembership}`), "ADMIN");
  // 되돌려 이후 시나리오의 전제(Eve는 Acme MEMBER 아님/ADMIN)를 단순화하지 않고, Eve를 Acme에서 뺀다(ADMIN은 remove 못함 → OWNER 외 불가를 확인).
  const removeAdmin = await cat().apply({ apply: "Membership.remove", target: { ids: [eveMembership] } } as never, { key: "td-m-remove-admin-row" });
  assert.equal(removeAdmin.ok, false, "remove는 role = MEMBER 행만(ADMIN 행은 from이 막는다)");
  assert.equal(sql(`SELECT count(*) FROM ${schema}.membership WHERE id=${eveMembership}`), "1");
});

await scenario("D3-draft-visibility-owner-admin-only-published-to-members", async () => {
  const made = await create(ben, { org: ACME, owner: BEN, title: "ben-draft", body: "wip", internalNote: "ben-private", state: "DRAFT", version: 0 }, "td-doc-1");
  ok(made, "Ben의 초안 생성");
  s.draft = made.created[0]!.id;
  assert.ok((await titles(ben)).includes("ben-draft"), "작성자는 자기 초안을 본다");
  assert.ok((await titles(cat)).includes("ben-draft"), "조직 ADMIN은 본다");
  assert.ok((await titles(ann)).includes("ben-draft"), "조직 OWNER는 본다");
  assert.ok(!(await titles(fay)).includes("ben-draft"), "같은 조직 MEMBER도 남의 초안은 못 본다");
  assert.ok(!(await titles(dan)).includes("ben-draft"), "다른 조직 OWNER는 못 본다");
  const published = await ben().apply({ apply: "Doc.publish", target: { ids: [s.draft!] } } as never, { key: "td-doc-1-pub" });
  ok(published, "작성자 게시");
  assert.ok((await titles(fay)).includes("ben-draft"), "게시되면 조직 MEMBER가 본다");
  assert.ok(!(await titles(dan)).includes("ben-draft"), "게시돼도 다른 조직은 못 본다");
  const unpubByMember = await fay().apply({ apply: "Doc.unpublish", target: { ids: [s.draft!] } } as never, { key: "td-doc-1-unpub-fay" });
  denied(unpubByMember, "WIRE.FORBIDDEN", "MEMBER의 게시 취소");
  const unpubByAdmin = await cat().apply({ apply: "Doc.unpublish", target: { ids: [s.draft!] } } as never, { key: "td-doc-1-unpub-cat" });
  ok(unpubByAdmin, "ADMIN의 게시 취소");
  assert.equal(sql(`SELECT state FROM ${schema}.doc WHERE id=${s.draft}`), "DRAFT");
});

await scenario("D4-multi-org-user-role-is-per-org", async () => {
  // Ann은 Acme OWNER, Beta MEMBER. Beta에서는 OWNER 권한이 없다.
  const beta = (await ann().read({ read: "Doc", select: ["id", "title"], filter: [{ field: "org", op: "eq", value: BETA }] })).rows.map((r) => r.title);
  assert.deepEqual(beta, ["beta-public"], "Beta에서 Ann은 게시 문서만");
  const dansDraft = await ann().apply({ apply: "Doc.edit", target: { ids: [BETA_DRAFT] }, values: { title: "pwn" }, expect: { version: 0 } } as never, { key: "td-x-edit" });
  assert.equal(dansDraft.ok, false);
  const unpub = await ann().apply({ apply: "Doc.unpublish", target: { ids: [BETA_PUBLIC] } } as never, { key: "td-x-unpub" });
  denied(unpub, "WIRE.FORBIDDEN", "Acme OWNER가 Beta 문서 게시 취소");
  const own = await create(ann, { org: BETA, owner: ANN, title: "ann-in-beta", body: "x", internalNote: null, state: "DRAFT", version: 0 }, "td-ann-beta");
  ok(own, "Beta MEMBER가 Beta에 문서 생성");
  const notMember = await create(ann, { org: BETA, owner: DAN, title: "as-dan", body: "x", internalNote: null, state: "DRAFT", version: 0 }, "td-ann-as-dan");
  assert.equal(notMember.ok, false, "owner를 남으로 위조");
  const outsider = await create(fay, { org: BETA, owner: FAY, title: "fay-in-beta", body: "x", internalNote: null, state: "DRAFT", version: 0 }, "td-fay-beta");
  assert.equal(outsider.ok, false, "소속 아닌 조직에 문서 생성");
  assert.equal(sql(`SELECT count(*) FROM ${schema}.doc WHERE title IN ('pwn','as-dan','fay-in-beta')`), "0");
});

await scenario("D5-share-view-edit-revoke-and-no-cross-org-share", async () => {
  const made = await create(ben, { org: ACME, owner: BEN, title: "ben-shared", body: "v0", internalNote: "ben-only", state: "DRAFT", version: 0 }, "td-doc-2");
  ok(made, "공유할 문서");
  s.shared = made.created[0]!.id;
  assert.ok(!(await titles(fay)).includes("ben-shared"));
  const share = await ben().bundle({ atomic: [{ create: "DocShare", values: { doc: s.shared!, grantee: FAY, grant: "VIEW" } }] }, { key: "td-share-1" });
  ok(share, "VIEW 공유");
  s.shareFay = share.created[0]!.id;
  const createdGrant = await ben().read({ read: "DocShare", select: ["grant"], filter: [{ field: "doc", op: "eq", value: s.shared! }] });
  assert.deepEqual(createdGrant.rows.map((row) => row.grant), ["VIEW"], "예약어 필드 grant로 VIEW 공유 생성·조회");
  assert.ok((await titles(fay)).includes("ben-shared"), "공유받은 Fay가 초안을 읽는다");
  const viewEdit = await fay().apply({ apply: "Doc.edit", target: { ids: [s.shared!] }, values: { title: "fay-edit" }, expect: { version: 0 } } as never, { key: "td-share-fay-edit-1" });
  denied(viewEdit, "WIRE.FORBIDDEN", "VIEW 공유로 수정");
  const regrant = await ben().apply({ apply: "DocShare.regrant", target: { ids: [s.shareFay!] }, values: { grant: "EDIT" } } as never, { key: "td-share-regrant" });
  ok(regrant, "EDIT로 올림");
  const changedGrant = await ben().read({ read: "DocShare", select: ["grant"], filter: [{ field: "doc", op: "eq", value: s.shared! }] });
  assert.deepEqual(changedGrant.rows.map((row) => row.grant), ["EDIT"], "예약어 필드 grant 수정·조회");
  const edited = await fay().apply({ apply: "Doc.edit", target: { ids: [s.shared!] }, values: { title: "ben-shared v1" }, expect: { version: 0 } } as never, { key: "td-share-fay-edit-2" });
  ok(edited, "EDIT 공유로 수정");
  const stale = await ben().apply({ apply: "Doc.edit", target: { ids: [s.shared!] }, values: { title: "stale" }, expect: { version: 0 } } as never, { key: "td-share-ben-stale" });
  denied(stale, "WIRE.CONFLICT", "낡은 version");
  assert.equal(sql(`SELECT title||'/'||version FROM ${schema}.doc WHERE id=${s.shared}`), "ben-shared v1/1");
  // 다른 조직 사용자(Dan)에게는 공유할 수 없고, 소유자가 아닌 MEMBER(Fay)가 제3자에게 공유할 수도 없다.
  const cross = await ben().bundle({ atomic: [{ create: "DocShare", values: { doc: s.shared!, grantee: DAN, grant: "VIEW" } }] }, { key: "td-share-cross" });
  assert.equal(cross.ok, false, "조직 밖 사용자에게 공유");
  const reshare = await fay().bundle({ atomic: [{ create: "DocShare", values: { doc: s.shared!, grantee: CAT, grant: "EDIT" } }] }, { key: "td-share-reshare" });
  assert.equal(reshare.ok, false, "EDIT 공유자는 재공유 못 한다");
  const self = await ben().bundle({ atomic: [{ create: "DocShare", values: { doc: s.shared!, grantee: BEN, grant: "EDIT" } }] }, { key: "td-share-self" });
  assert.equal(self.ok, false, "자기 자신에게 공유");
  assert.equal(sql(`SELECT count(*) FROM ${schema}.doc_share WHERE doc_id=${s.shared}`), "1");
  const revoked = await ben().apply({ apply: "DocShare.revoke", target: { ids: [s.shareFay!] } } as never, { key: "td-share-revoke" });
  ok(revoked, "공유 해제");
  assert.ok(!(await titles(fay)).includes("ben-shared v1"), "해제 즉시 Fay는 못 읽는다");
  const afterRevoke = await fay().apply({ apply: "Doc.edit", target: { ids: [s.shared!] }, values: { title: "late" }, expect: { version: 1 } } as never, { key: "td-share-fay-late" });
  denied(afterRevoke, "WIRE.MISSING_TARGET", "해제 뒤 수정은 존재 은폐");
});

await scenario("D6-field-restriction-note-and-email", async () => {
  const note = async (who: () => ReturnType<typeof as>) =>
    (await who().read({ read: "Doc", select: ["id", "internalNote"], filter: [{ field: "id", op: "eq", value: s.shared! }] })).rows[0]?.internalNote;
  assert.equal(await note(ben), "ben-only", "소유자는 본다");
  assert.equal(await note(cat), "ben-only", "조직 ADMIN은 본다");
  // Fay에게 다시 EDIT 공유를 줘도 메모는 가려지고, 메모 쓰기 전이(annotate)도 못 쓴다.
  const share = await ben().bundle({ atomic: [{ create: "DocShare", values: { doc: s.shared!, grantee: FAY, grant: "EDIT" } }] }, { key: "td-share-again" });
  ok(share, "다시 EDIT 공유");
  assert.equal(await note(fay), null, "공유받은 편집자도 내부 메모는 null");
  const annotate = await fay().apply({ apply: "Doc.annotate", target: { ids: [s.shared!] }, values: { internalNote: "fay-writes" }, expect: { version: 1 } } as never, { key: "td-annotate-fay" });
  denied(annotate, "WIRE.FORBIDDEN", "편집자의 메모 쓰기");
  const byAdmin = await cat().apply({ apply: "Doc.annotate", target: { ids: [s.shared!] }, values: { internalNote: "admin-note" }, expect: { version: 1 } } as never, { key: "td-annotate-cat" });
  ok(byAdmin, "ADMIN의 메모 쓰기");
  assert.equal(await note(ben), "admin-note");
  const users = await fay().read({ read: "User", select: ["id", "email"], sort: [{ field: "id", dir: "asc" }] });
  assert.deepEqual(users.rows.map((r) => r.email), [null, null, null, null, null, "fay@example.test"], "이메일은 본인 것만");
  const hidden = sql(`SELECT internal_note FROM ${schema}.doc WHERE id=${s.shared}`);
  assert.equal(hidden, "admin-note", "DB에는 값이 있고 읽기 경로만 가린다");
});

await scenario("D7-relation-read-and-aggregate-respect-visibility", async () => {
  const doc = (await ben().read({ read: "Doc", select: ["id", "title", "editorCount", { owner: { select: ["id", "name"] } }, { shares: { select: ["id", "grantee", "grant"], limit: 10 } }],
    filter: [{ field: "id", op: "eq", value: s.shared! }] } as never)).rows[0] as any;
  assert.equal(doc.owner.name, "Ben");
  assert.equal(doc.editorCount, 1, "EDIT 공유 1건");
  assert.equal(doc.shares.length, 1);
  assert.equal(doc.shares[0].grant, "EDIT", "관계 조회에서 예약어 필드 grant 확인");
  const forFay = (await fay().read({ read: "Doc", select: ["id", "editorCount", { shares: { select: ["id", "grantee", "grant"], limit: 10 } }],
    filter: [{ field: "id", op: "eq", value: s.shared! }] } as never)).rows[0] as any;
  assert.equal(forFay.shares.length, 1, "Fay는 자기 공유 행만");
  assert.equal(forFay.shares[0].grantee, FAY);
  // 읽을 수 없는 문서의 집계는 문서 행 자체가 안 보여 값이 새지 않는다.
  const hiddenRows = (await fay().read({ read: "Doc", select: ["id", "editorCount"], filter: [{ field: "id", op: "eq", value: s.draft! }] } as never)).rows;
  assert.equal(hiddenRows.length, 0, "남의 초안은 집계 값과 함께 안 보인다");
  const afterSecond = await ben().bundle({ atomic: [{ create: "DocShare", values: { doc: s.shared!, grantee: CAT, grant: "EDIT" } }] }, { key: "td-share-cat" });
  ok(afterSecond, "두 번째 EDIT 공유");
  const count = (await ben().read({ read: "Doc", select: ["editorCount"], filter: [{ field: "id", op: "eq", value: s.shared! }] } as never)).rows[0] as any;
  assert.equal(count.editorCount, 2);
  assert.equal(sql(`SELECT count(*) FROM ${schema}.doc_share WHERE doc_id=${s.shared} AND "grant"='EDIT'`), "2", "집계가 DB의 예약어 grant 컬럼 행 수와 같다");
});

await scenario("D8-leave-and-remove-membership-revokes-access", async () => {
  // Fay가 Acme를 떠나면 공유 행은 남아도 조직 밖이라 목록에서 사라지는지 본다(공유 grantee 조건만으로는 계속 읽힌다).
  const mine = sql(`SELECT id FROM ${schema}.membership WHERE org_id=1 AND user_id=${FAY}`);
  const left = await fay().apply({ apply: "Membership.leave", target: { ids: [mine] } } as never, { key: "td-leave" });
  ok(left, "Fay 탈퇴");
  const stillShared = (await titles(fay)).includes("ben-shared v1");
  assert.equal(stillShared, false, "조직을 떠나면 공유 행이 남아 있어도 읽히지 않는다");
  assert.equal(sql(`SELECT count(*) FROM ${schema}.doc_share WHERE doc_id=${s.shared} AND grantee_id=${FAY}`), "1", "공유 행은 남아 있다(소속이 읽기를 막는다)");
  const asLeaver = await fay().apply({ apply: "Doc.edit", target: { ids: [s.shared!] }, values: { title: "after-leave" }, expect: { version: 3 } } as never, { key: "td-leave-edit" });
  denied(asLeaver, "WIRE.MISSING_TARGET", "탈퇴 뒤 수정");
  const other = await ben().apply({ apply: "Membership.leave", target: { ids: [sql(`SELECT id FROM ${schema}.membership WHERE org_id=1 AND user_id=${BEN}`)] } } as never, { key: "td-leave-ben" });
  ok(other, "Ben도 본인 행으로 탈퇴 가능");
  const notMine = await cat().apply({ apply: "Membership.leave", target: { ids: [sql(`SELECT id FROM ${schema}.membership WHERE org_id=2 AND user_id=${EVE}`)] } } as never, { key: "td-leave-others" });
  assert.equal(notMine.ok, false, "남의 멤버십 탈퇴는 거부");
});

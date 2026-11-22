// D1: `service check`가 통과시킨 정의가 런타임에서 지원되지 않거나 부정확한 경우, 그리고 실행 가능한데 정적으로 거부되는 경우를
// 현재 HEAD에서 반례로 찾은 표. 정의 조각마다 `service check` 결과와 PG17에서 실제로 돌린 런타임 결과를 기록한다.
// 기대 값은 product/tests/fixtures/definition-matrix.expected.json(골든)이다. 동작이 바뀌면 이 시험이 실패하고,
// 의도한 변경이면 AIP_UPDATE_MATRIX=1로 골든을 다시 쓴다. 표는 plan-docs/stabilization-20261122/track-d.md에 옮겨 적었다.
//
//   cargo build -p aip-cli && node --test --test-concurrency=1 product/tests/semantic-definition-matrix.test.mjs
import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { after, test } from "node:test";
import { startFixture, cleanupTemp, root, run, binary } from "../test-support/service-fixture.mjs";

const golden = resolve(dirname(fileURLToPath(import.meta.url)), "fixtures/definition-matrix.expected.json");
after(() => cleanupTemp());

const PRE = `resource Member { fields { id: Id } rows read when id = actor }\nactor Member\n`;
const READ = (extra = "") => `expose read { select id${extra}; sort id; budget { rows 20; depth 2; deadline 2s; cost 1000 } }`;
const SEED_M = "INSERT INTO ${S}.member(id) VALUES (1);";
const cases = [];
const add = (id, title, resources, seed, runtime, options = {}) => cases.push({ id, title, definition: PRE + resources, seed: SEED_M + (seed ?? ""), runtime, options });
const rd = (q) => async (f) => { const r = await f.post("/read", { query: q }, { token: "user" }); return r.body.ok ? `ok rows=${JSON.stringify(r.body.rows).slice(0, 120)}` : `${r.body.code}`; };
const ap = (req) => async (f) => { const r = await f.post("/apply", { request: req, key: "p-" + Math.random().toString(36).slice(2, 12) + "0000" }, { token: "user" }); return r.body.ok ? `ok ${JSON.stringify({changed: r.body.changed, created: r.body.created}).slice(0,100)}` : `${r.body.code}`; };

add("Int-unbounded-beyond-2^53", "Int without a range holding bigint values above the JS safe range", `resource T { fields { id: Id; n: Int } rows read when true ${READ(", n")} }`, "INSERT INTO ${S}.t(id,n) VALUES (1,9007199254740993),(2,9223372036854775807);", async (f) => {
  const raw = await fetch(f.base + "/read", { method: "POST", headers: { "content-type": "application/json", authorization: "Bearer " + f.tokens.user, "x-aip-contract": f.fingerprint }, body: JSON.stringify({ query: { read: "T", select: ["id", "n"], sort: [{ field: "id" }] } }) });
  const text = await raw.text();
  const exact = text.match(/"rows":(\[.*?\])/u)?.[1];
  const { connect } = await import(join(root, "product/sdk/index.ts"));
  const { contract } = await import(join(f.dir, "binding.ts"));
  const ts = await connect(f.base, f.tokens.user, contract).read({ read: "T", select: ["id", "n"] }).then(() => "ok", (e) => e.code);
  const py = run("python3", ["-c", `import sys,json; sys.path.insert(0, ${JSON.stringify(join(root, "product/python"))}); from aip_sdk import connect; c = connect(${JSON.stringify(f.base)}, ${JSON.stringify(f.tokens.user)}, ${JSON.stringify(f.fingerprint)}); print(json.dumps(c.read({"read":"T","select":["id","n"],"sort":[{"field":"id"}]})))`]);
  return `server=${exact} | ts=${ts} | python=${py}`;
}, { sdkImport: join(root, "product/sdk/index.ts"), python: true });
add("Int-range-bigint-bounds", "Int(0..9223372036854775807) beyond safe integer", `resource T { fields { id: Id; n: Int(0..9223372036854775807) } rows read when true ${READ(", n")} }`, "", async (f) => "check-only");
add("Decimal-min-max", "Decimal aggregate min/max with groupKey", `resource G { fields { id: Id; owner: Member } rows read when true expose read { select id, top; sort id; budget { rows 5; depth 1; deadline 1s; cost 100 } }
  aggregate top: Decimal(10,2)? { source V; sourceAccess everyone(actor); groupKey g; callerFilter none; rowOutput none; release max(amt) } }
access everyone(a: Member) = true
resource V { fields { id: Id; g: G; amt: Decimal(10,2) } rows read when true }`, "", async (f) => "check-only");
add("Decimal-sum", "Decimal sum per group", `access everyone(a: Member) = true
resource G { fields { id: Id; owner: Member } rows read when true
  aggregate total: Decimal(12,2) { source V; sourceAccess everyone(actor); groupKey g; callerFilter none; rowOutput none; release sum(amt) }
  expose read { select id, total; sort id; budget { rows 5; depth 1; deadline 1s; cost 100 } } }
resource V { fields { id: Id; g: G; amt: Decimal(10,2) } rows read when true }`, "INSERT INTO ${S}.g(id,owner_id) VALUES (1,1); INSERT INTO ${S}.v(id,g_id,amt) VALUES (1,1,'1.10'),(2,1,'2.25');", rd({ read: "G", select: ["id", "total"] }));
add("Int-min-per-group", "Int min per group with an empty group", `access everyone(a: Member) = true
resource G { fields { id: Id; owner: Member } rows read when true
  aggregate low: Int? { source V; sourceAccess everyone(actor); groupKey g; callerFilter none; rowOutput none; release min(n) }
  expose read { select id, low; sort id; budget { rows 5; depth 1; deadline 1s; cost 100 } } }
resource V { fields { id: Id; g: G; n: Int(0..100) } rows read when true }`, "INSERT INTO ${S}.g(id,owner_id) VALUES (1,1),(2,1); INSERT INTO ${S}.v(id,g_id,n) VALUES (1,1,5),(2,1,3);", rd({ read: "G", select: ["id", "low"] }));
add("nullable-sort-cursor", "nullable sort field together with cursor paging", `resource T { fields { id: Id; n: Int(0..9)? } rows read when true expose read { select id, n; sort n, id; budget { rows 5; depth 1; deadline 1s; cost 100; cursor } } }`, "", async () => "check-only");
add("cursor-sort-not-selected", "cursor paging with a sort field that is not selected", `resource T { fields { id: Id; n: Int(0..9) } rows read when true expose read { select id; sort n, id; budget { rows 5; depth 1; deadline 1s; cost 100; cursor } } }`, "", async () => "check-only");
add("traverse-depth-below-need", "budget depth 1 with a traverse that needs depth 2", `resource C { fields { id: Id } rows read when true expose read { select id } }
resource T { fields { id: Id; c: C } rows read when true expose read { select id, c; traverse c { select id }; sort id; budget { rows 5; depth 1; deadline 1s; cost 100 } } }`, "INSERT INTO ${S}.c(id) VALUES (1); INSERT INTO ${S}.t(id,c_id) VALUES (1,1);", rd({ read: "T", select: ["id", { c: { select: ["id"] } }] }));
add("cost-too-low", "budget cost 1 on a plain read", `resource T { fields { id: Id; n: Int(0..9) } rows read when true expose read { select id, n; sort id; budget { rows 5; depth 1; deadline 1s; cost 1 } } }`, "INSERT INTO ${S}.t(id,n) VALUES (1,1);", rd({ read: "T", select: ["id", "n"] }));
add("deadline-1ms", "budget deadline 1ms", `resource T { fields { id: Id; n: Int(0..9) } rows read when true expose read { select id, n; sort id; budget { rows 5; depth 1; deadline 1ms; cost 100 } } }`, "INSERT INTO ${S}.t(id,n) VALUES (1,1);", async (f) => {
  // 1ms 한도는 질의 속도와 경쟁해 답이 ok일 수도 WIRE.DEADLINE_EXCEEDED일 수도 있다. 결과 집합만 기록한다.
  const seen = new Set();
  for (let i = 0; i < 6; i++) { const r = await f.post("/read", { query: { read: "T", select: ["id", "n"] } }, { token: "user" }); seen.add(r.body.ok ? "ok" : r.body.code); }
  return `answers within {ok, WIRE.DEADLINE_EXCEEDED}: ${[...seen].every((x) => x === "ok" || x === "WIRE.DEADLINE_EXCEEDED")}`;
});
add("rows-1-with-limit", "budget rows 1 with 3 rows present", `resource T { fields { id: Id } rows read when true expose read { select id; sort id; budget { rows 1; depth 1; deadline 1s; cost 100 } } }`, "INSERT INTO ${S}.t(id) VALUES (1),(2),(3);", rd({ read: "T", select: ["id"] }));
add("text-range-codepoints", "Text(1..3) with three emoji and with a combining mark", `resource T { fields { id: Id; s: Text(1..3) } rows read when true ${READ(", s")}
  transition set { allow true; from true; patch s } expose apply set { target id; bulk maxRows 1 } }`, "INSERT INTO ${S}.t(id,s) VALUES (1,'a');", async (f) => {
  const out = [];
  for (const [label, s] of [["3 emoji", "😀😀😀"], ["4 emoji", "😀😀😀😀"], ["e+combining x3", "ééé"], ["nul", "a\u0000b"]]) {
    const r = await f.post("/apply", { request: { apply: "T.set", target: { ids: ["1"] }, values: { s } }, key: "t-" + label.replace(/\W/g, "") + "00000" }, { token: "user" });
    out.push(`${label}=${r.body.ok ? "ok" : r.body.code}`);
  }
  return out.join(" ");
});
add("decimal-literal-forms", "Decimal(5,2) literals in to: 0, 0.00, \"0.0\"", `resource T { fields { id: Id; d: Decimal(5,2) } rows read when true ${READ(", d")} transition z { allow true; from true; to d = 0 } expose apply z { target id; bulk maxRows 1 } }`, "", async () => "check-only");
add("decimal-literal-string-short", "Decimal(5,2) literal \"0.5\" (fewer digits than scale)", `resource T { fields { id: Id; d: Decimal(5,2) } rows read when true ${READ(", d")} transition z { allow true; from true; to d = "0.5" } expose apply z { target id; bulk maxRows 1 } }`, "INSERT INTO ${S}.t(id,d) VALUES (1,'1.00');", async (f) => { const a = await ap({ apply: "T.z", target: { ids: ["1"] } })(f); const r = await rd({ read: "T", select: ["d"] })(f); return a + " | " + r; });
add("arith-on-nullable", "f = f + 1 on a nullable Int field", `resource T { fields { id: Id; n: Int(0..9)? } rows read when true ${READ(", n")} transition inc { allow true; from true; to n = n + 1 } expose apply inc { target id; bulk maxRows 1 } }`, "", async () => "check-only");
add("arith-overflow-range", "n = n + 1 on Int(0..2) at 2", `resource T { fields { id: Id; n: Int(0..2) } rows read when true ${READ(", n")} transition inc { allow true; from true; to n = n + 1 } expose apply inc { target id; bulk maxRows 1 } }`, "INSERT INTO ${S}.t(id,n) VALUES (1,2);", ap({ apply: "T.inc", target: { ids: ["1"] } }));
add("arith-decimal-int-term", "Decimal field += Int literal", `resource T { fields { id: Id; d: Decimal(6,2) } rows read when true ${READ(", d")} transition inc { allow true; from true; to d = d + 1 } expose apply inc { target id; bulk maxRows 1 } }`, "INSERT INTO ${S}.t(id,d) VALUES (1,'1.50');", async (f) => { const a = await ap({ apply: "T.inc", target: { ids: ["1"] } })(f); const r = await rd({ read: "T", select: ["d"] })(f); return a + " | " + r; });
add("arith-this-field", "n = n + this.m", `resource T { fields { id: Id; n: Int(0..99); m: Int(1..9) } rows read when true ${READ(", n")} transition inc { allow true; from true; to n = n + this.m } expose apply inc { target id; bulk maxRows 1 } }`, "INSERT INTO ${S}.t(id,n,m) VALUES (1,5,3);", async (f) => { const a = await ap({ apply: "T.inc", target: { ids: ["1"] } })(f); const r = await rd({ read: "T", select: ["n"] })(f); return a + " | " + r; });
add("arith-this-field-zero-able", "n = n + this.m where m may be 0 or negative", `resource T { fields { id: Id; n: Int(0..99); m: Int(-5..9) } rows read when true ${READ(", n")} transition inc { allow true; from true; to n = n + this.m } expose apply inc { target id; bulk maxRows 1 } }`, "INSERT INTO ${S}.t(id,n,m) VALUES (1,5,-3);", async (f) => { const a = await ap({ apply: "T.inc", target: { ids: ["1"] } })(f); const r = await rd({ read: "T", select: ["n"] })(f); return a + " | " + r; });
add("input-int-range-includes-zero", "transition input Int(0..5)", `resource T { fields { id: Id; n: Int(0..99) } rows read when true ${READ(", n")} transition inc { input { k: Int(0..5) }; allow true; from true; to n = n + input.k } expose apply inc { target id; bulk maxRows 1 } }`, "", async () => "check-only");
add("input-in-allow-compare-hidden", "input compared with a field-read-policy field in allow", `resource T { fields { id: Id; owner: Member; n: Int(0..99); s: Int(0..9) } rows read when true field s read when owner = actor ${READ(", n")} transition t { input { k: Int(1..5) }; allow input.k = s; from true; to n = n + input.k } expose apply t { target id; bulk maxRows 1 } }`, "", async () => "check-only");
add("compose-with-input", "compose step naming a transition with input", `resource T { fields { id: Id; n: Int(0..99) } rows read when true ${READ(", n")} transition inc { input { k: Int(1..5) }; allow true; from true; to n = n + input.k } expose compose { bulk maxRows 2; transitions inc } }`, "", async () => "check-only");
add("apply-where-target", "apply with target where on a filter the read exposes, and on one it does not", `resource T { fields { id: Id; n: Int(0..9); tag: Text(1..5); other: Text(1..5) } rows read when true expose read { select id, n; filter tag.eq; sort id; budget { rows 20; depth 1; deadline 2s; cost 1000 } } transition t { allow true; from true; to n = 1 } expose apply t { target where; bulk maxRows 5 } }`, "INSERT INTO ${S}.t(id,n,tag,other) VALUES (1,0,'a','x'),(2,0,'b','x');", async (f) => { const out = []; for (const field of ["tag", "other"]) { const r = await f.post("/apply", { request: { apply: "T.t", target: { where: [{ field, op: "eq", value: field === "tag" ? "a" : "x" }] } }, key: `w-where-${field}-0001` }, { token: "user" }); out.push(`${field}=${r.body.ok ? "ok changed=" + r.body.changed : r.body.code}`); } return out.join(" "); });
add("apply-maxRows-over-bundle-cap", "expose apply bulk maxRows 5000 (bundle row cap is 1000)", `resource T { fields { id: Id; n: Int(0..9) } rows read when true ${READ(", n")} transition t { allow true; from true; to n = 1 } expose apply t { target id; bulk maxRows 5000 } }`, "INSERT INTO ${S}.t(id,n) SELECT g,0 FROM generate_series(1,1100) g;", async (f) => {
  const ids = Array.from({ length: 1100 }, (_, i) => String(i + 1));
  const direct = await f.post("/apply", { request: { apply: "T.t", target: { ids } }, key: "w-big0000000001" }, { token: "user" });
  const bundle = await f.post("/apply", { request: { atomic: [{ apply: "T.t", target: { ids } }] }, key: "w-big0000000002" }, { token: "user" });
  return `direct=${direct.body.ok ? "ok" : direct.body.code} bundle=${bundle.body.ok ? "ok" : bundle.body.code}`; });
add("enum-in-filter", "enum field with filter in and a transition assigning an enum literal", `enum S { A, B, C }
resource T { fields { id: Id; s: S } rows read when true expose read { select id, s; filter s.eq, s.in; sort id; budget { rows 5; depth 1; deadline 1s; cost 100 } } }`, "INSERT INTO ${S}.t(id,s) VALUES (1,'A'),(2,'B');", rd({ read: "T", select: ["id", "s"], filter: [{ field: "s", op: "in", value: ["A", "C"] }] }));
add("filter-isNull-nullable", "filter isNull on a nullable Text", `resource T { fields { id: Id; n: Text(1..5)? } rows read when true expose read { select id, n; filter n.isNull; sort id; budget { rows 5; depth 1; deadline 1s; cost 100 } } }`, "INSERT INTO ${S}.t(id,n) VALUES (1,NULL),(2,'x');", rd({ read: "T", select: ["id", "n"], filter: [{ field: "n", op: "isNull", value: true }] }));
add("filter-like-percent", "Text filter like with a literal percent and underscore", `resource T { fields { id: Id; n: Text(1..9) } rows read when true expose read { select id, n; filter n.like, n.contains, n.prefix; sort id; budget { rows 5; depth 1; deadline 1s; cost 100 } } }`, "INSERT INTO ${S}.t(id,n) VALUES (1,'50%off'),(2,'50xoff'),(3,'a_b');", async (f) => { const out = []; for (const [op, value] of [["contains", "%"], ["prefix", "50%"], ["like", "50\\%%"], ["contains", "_"]]) { const r = await f.post("/read", { query: { read: "T", select: ["id"], filter: [{ field: "n", op, value }] } }, { token: "user" }); out.push(`${op}(${value})=${r.body.ok ? r.body.rows.map(x => x.id).join(",") : r.body.code}`); } return out.join(" "); });
add("sort-bool-desc-null", "sort by a nullable Bool, ASC then DESC", `resource T { fields { id: Id; b: Bool? } rows read when true expose read { select id, b; sort b, id; budget { rows 5; depth 1; deadline 1s; cost 100 } } }`, "INSERT INTO ${S}.t(id,b) VALUES (1,NULL),(2,true),(3,false);", async (f) => { const a = await f.post("/read", { query: { read: "T", select: ["id"], sort: [{ field: "b" }] } }, { token: "user" }); const d = await f.post("/read", { query: { read: "T", select: ["id"], sort: [{ field: "b", dir: "desc" }] } }, { token: "user" }); return `asc=${a.body.rows?.map(r => r.id)} desc=${d.body.rows?.map(r => r.id)}`; });
add("row-policy-null-actor", "rows read when actor = null on a non-null actor reference", `resource T { fields { id: Id } rows read when actor = null expose read { select id; sort id; budget { rows 5; depth 1; deadline 1s; cost 100 } } }`, "", async () => "check-only");
add("traverse-many", "1:N traverse with limit and a child row policy", `resource P { fields { id: Id } rows read when true expose read { select id; traverse kids via K.p { select id; limit 2 }; sort id; budget { rows 5; depth 2; deadline 1s; cost 100 } } }
resource K { fields { id: Id; p: P; hidden: Bool } rows read when hidden = false expose read { select id } }`, "INSERT INTO ${S}.p(id) VALUES (1); INSERT INTO ${S}.k(id,p_id,hidden) VALUES (1,1,false),(2,1,true),(3,1,false),(4,1,false);", rd({ read: "P", select: ["id", { kids: { select: ["id"] } }] }));
add("unique-with-null", "unique on a nullable column, two NULL rows", `resource T { fields { id: Id; code: Text(1..5)? } rows read when true ${READ(", code")} unique code expose create { allow true; fields code } }`, "", async (f) => { const a = await f.post("/apply", { request: { atomic: [{ create: "T", values: { code: null } }, { create: "T", values: { code: null } }] }, key: "u-null000000001" }, { token: "user" }); const b = await f.post("/apply", { request: { atomic: [{ create: "T", values: { code: "x" } }, { create: "T", values: { code: "x" } }] }, key: "u-null000000002" }, { token: "user" }); return `nullx2=${a.body.ok ? "ok" : a.body.code} xx=${b.body.ok ? "ok" : b.body.code}`; });
add("delete-transition", "transition delete then read", `resource T { fields { id: Id } rows read when true ${READ()} transition drop { allow true; from true; delete } expose apply drop { target id; bulk maxRows 2 } }`, "INSERT INTO ${S}.t(id) VALUES (1),(2);", async (f) => { const a = await ap({ apply: "T.drop", target: { ids: ["1"] } })(f); const r = await rd({ read: "T", select: ["id"] })(f); return a + " | " + r; });
add("latest-effect-null", "update effect with latest ... when no candidate row exists (no otherwise / otherwise this.created)", `resource SalesOrder { fields { id: Id; owner: Member; refreshed: Bool; created: Time? } rows read when owner = actor ${READ(", refreshed")}
  transition refresh { allow owner = actor; from refreshed = false; to refreshed = true
    update Summary where order = this.id {
      label = latest Item candidate where candidate.order = this.id and candidate.active = true order by candidate.created select candidate.label
      created = latest Item candidate where candidate.order = this.id and candidate.active = true order by candidate.created select candidate.created otherwise this.created
    } }
  expose apply refresh { target id; bulk maxRows 1 } }
resource Summary { fields { id: Id; order: SalesOrder; label: Text?; created: Time? } rows read when order.owner = actor ${READ(", label, created")} }
resource Item { fields { id: Id; order: SalesOrder; active: Bool; label: Text; created: Time? } rows read when order.owner = actor }`, "INSERT INTO ${S}.sales_order(id,owner_id,refreshed,created) VALUES (1,1,false,'2024-01-01T00:00:00Z'); INSERT INTO ${S}.summary(id,order_id,label,created) VALUES (1,1,'stale','2020-01-01T00:00:00Z');", async (f) => { const a = await ap({ apply: "SalesOrder.refresh", target: { ids: ["1"] } })(f); const r = await rd({ read: "Summary", select: ["label", "created"] })(f); return a + " | " + r; });
add("shared-primary-key-sort-id", "id: Member (1:1 shared primary key) with `sort id`", `resource Profile { fields { id: Member; bio: Text(1..20)? } rows read when id = actor ${READ(", bio")} }`, "", async () => "check-only");
add("shared-primary-key-no-sort", "the same resource without a sort list: read and patch work", `resource Profile { fields { id: Member; bio: Text(1..20)? } rows read when id = actor expose read { select id, bio; budget { rows 20; depth 2; deadline 2s; cost 1000 } } transition set { allow id = actor; from true; patch bio } expose apply set { target id; bulk maxRows 1 } }`, "INSERT INTO ${S}.profile(id,bio) VALUES (1,'x');", async (f) => { const a = await ap({ apply: "Profile.set", target: { ids: ["1"] }, values: { bio: "y" } })(f); const r = await rd({ read: "Profile", select: ["id", "bio"] })(f); return a + " | " + r; });
add("sort-on-reference-field", "sort by a reference field", `resource C { fields { id: Id } rows read when true expose read { select id } }
resource T { fields { id: Id; c: C } rows read when true expose read { select id, c; sort c, id; budget { rows 5; depth 2; deadline 1s; cost 100 } } }`, "", async () => "check-only");
add("filter-decimal-in", "Decimal filter with the in operator", `resource T { fields { id: Id; d: Decimal(5,2) } rows read when true expose read { select id, d; filter d.in; sort id; budget { rows 5; depth 1; deadline 1s; cost 100 } } }`, "", async () => "check-only");
add("select-same-field-twice", "the same field selected twice in one read", `resource T { fields { id: Id; n: Int(0..9) } rows read when true ${READ(", n")} }`, "INSERT INTO ${S}.t(id,n) VALUES (1,1);", rd({ read: "T", select: ["id", "n", "n"] }));
add("time-now-policy", "rows read when t >= now", `resource T { fields { id: Id; t: Time } rows read when t >= now ${READ(", t")} }`, "INSERT INTO ${S}.t(id,t) VALUES (1, now() + interval '1 hour'),(2, now() - interval '1 hour');", async (f) => { const r = await f.post("/read", { query: { read: "T", select: ["id"] } }, { token: "user" }); return r.body.ok ? `ok ids=${r.body.rows.map((x) => x.id)}` : r.body.code; });
add("json-field", "Json field read", `resource T { fields { id: Id; j: Json } rows read when true ${READ(", j")} }`, "INSERT INTO ${S}.t(id,j) VALUES (1,'{\"a\":[1,2.50,null]}');", rd({ read: "T", select: ["id", "j"] }));
add("email-url-date", "Email/Url/Date scalars round trip", `resource T { fields { id: Id; e: Email; u: Url; d: Date } rows read when true ${READ(", e, u, d")} }`, "INSERT INTO ${S}.t(id,e,u,d) VALUES (1,'a@b.co','https://x.y/z','2024-02-29');", rd({ read: "T", select: ["id", "e", "u", "d"] }));
add("check-and-invariant", "check clause and limit/invariant at commit", `limit twoOpen on T = atMost 2 where open = true
resource T { fields { id: Id; owner: Member; open: Bool } rows read when true ${READ(", open")} invariant twoOpen per owner expose create { allow true; fields owner, open } }`, "", async (f) => { const out = []; for (let i = 0; i < 3; i++) { const r = await f.post("/apply", { request: { atomic: [{ create: "T", values: { owner: "1", open: true } }] }, key: "inv-000000000" + i }, { token: "user" }); out.push(r.body.ok ? "ok" : r.body.code); } return out.join(","); });
add("read-extension-undeclared", "read extension without access declarations", `resource T { fields { id: Id } rows read when true ${READ()} extension read peek { input { x: Int(0..9) } output { y: Int(0..9) } effect none deadline 1s implementation "x.peek" } }`, "", async () => "check-only");


function checkOutcome(definition) {
  const dir = mkdtempSync(join(tmpdir(), "aip-matrix-"));
  const path = join(dir, "d.aip");
  writeFileSync(path, definition);
  const result = run(binary, ["service", "check", path], { allowFail: true });
  if (result.status === 0) return "pass";
  const lines = `${result.stdout}\n${result.stderr}`.trim().split("\n");
  let parsed;
  for (const line of lines.reverse()) { try { parsed = JSON.parse(line); break; } catch { /* next line */ } }
  const codes = (parsed?.diagnostics ?? []).map((d) => d.code);
  return `fail ${codes.length ? [...new Set(codes)].join(",") : (parsed?.code ?? "unparsed")}`;
}

async function measure() {
  const table = {};
  for (const c of cases) {
    const check = checkOutcome(c.definition);
    let runtime = "-";
    if (check === "pass" && c.runtime) {
      const f = await startFixture({ definition: c.definition, actors: { user: 1 }, seed: c.seed, python: c.options.python ?? false, sdkImport: c.options.sdkImport });
      try { runtime = await c.runtime(f); } finally { await f.stop(); }
    }
    table[c.id] = { title: c.title, check, runtime };
  }
  return table;
}

test("definition matrix: every fragment keeps the recorded check result and PG17 runtime result", { timeout: 600_000 }, async () => {
  const measured = await measure();
  if (process.env.AIP_UPDATE_MATRIX === "1") {
    writeFileSync(golden, JSON.stringify(measured, null, 2) + "\n");
    return;
  }
  const expected = JSON.parse(readFileSync(golden, "utf8"));
  assert.deepEqual(Object.keys(measured), Object.keys(expected), "the same fragments");
  const differences = Object.keys(measured).filter((id) => measured[id].check !== expected[id].check || measured[id].runtime !== expected[id].runtime);
  assert.deepEqual(differences.map((id) => [id, measured[id], expected[id]]), [], "check/runtime results changed (AIP_UPDATE_MATRIX=1 rewrites the golden when intended)");
});

// ---------------------------------------------------------------------------------------------------------------------------
// 핵심 파일(sema.rs 등) 수정이 필요한 결함의 재현 시험. lead의 수정 허가 전이라 todo로 둔다: 지금은 실패하고, 고치면 통과한다.
// node:test는 todo 시험이 실패해도 종료 코드를 바꾸지 않는다.
// ---------------------------------------------------------------------------------------------------------------------------
test("a budget cost the read can never fit is refused at check time", { todo: "core-file-wait: sema.rs budget check (track-d.md 핵심 파일 대기 목록 C1)" }, () => {
  const definition = PRE + `resource T { fields { id: Id; n: Int(0..9) } rows read when true expose read { select id, n; sort id; budget { rows 5; depth 1; deadline 1s; cost 1 } } }`;
  assert.notEqual(checkOutcome(definition), "pass", "a definition whose own read always answers WIRE.COST_EXCEEDED must not pass service check");
});

test("a shared primary key can be sorted by id like any other identity", { todo: "core-file-wait: sema.rs sortable types (track-d.md 핵심 파일 대기 목록 C3, 의도 확인 필요)" }, () => {
  const definition = PRE + `resource Profile { fields { id: Member; bio: Text(1..20)? } rows read when id = actor expose read { select id, bio; sort id; budget { rows 20; depth 2; deadline 2s; cost 1000 } } }`;
  assert.equal(checkOutcome(definition), "pass", "DEF.OP_TYPE_MISMATCH `id`는 정렬 불가 타입: the column is a bigint and orders like any id, and the read without a sort list serves it");
});

test("a transition term from a field whose range allows zero or negative values is refused at check time", { todo: "core-file-wait: sema.rs arithmetic term check (track-d.md 핵심 파일 대기 목록 C2)" }, () => {
  const definition = PRE + `resource T { fields { id: Id; n: Int(0..99); m: Int(-5..9) } rows read when true expose read { select id, n; sort id; budget { rows 5; depth 1; deadline 1s; cost 100 } } transition inc { allow true; from true; to n = n + this.m } expose apply inc { target id; bulk maxRows 1 } }`;
  assert.notEqual(checkOutcome(definition), "pass", "input terms need a positive range (DEF.BAD_RANGE); a this.field term whose range includes <= 0 is refused only at runtime (WIRE.BAD_VALUE)");
});

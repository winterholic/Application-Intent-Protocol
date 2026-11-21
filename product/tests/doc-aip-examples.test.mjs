// 문서의 ```aip 예제가 지금의 `aip service check`를 통과하는지 본다. 예제가 낡으면(예: expose apply에 bulk maxRows가 없음)
// 읽는 사람이 복사해 붙여도 DEF 오류가 나므로, 문서가 설명하는 계약과 검사기가 어긋난 것으로 취급한다.
// 선언 조각(extension·transition만 있는 블록)은 건너뛰고, actor 선언이 없는 완결 블록에는 최소 Member를 앞에 붙인다.
import assert from "node:assert/strict";
import { test } from "node:test";
import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const root = resolve(import.meta.dirname, "../..");
const binary = join(process.env.CARGO_TARGET_DIR || join(root, "target"), "debug/aip");
const docs = [
  "spec/definition-syntax.md",
  "README.md",
  ...readdirSync(join(root, "product")).filter((name) => name.endsWith(".md")).map((name) => `product/${name}`),
];
const PRELUDE = "actor Member\nresource Member { fields { id: Id } rows read when id = actor }\n";
const TOP_LEVEL = /^(resource|operation|enum|actor|predicate|access|limit)\b/;

function blocks(text) {
  const out = [];
  for (const match of text.matchAll(/```aip\n([\s\S]*?)```/g)) {
    out.push({ line: text.slice(0, match.index).split("\n").length, source: match[1] });
  }
  return out;
}

const dir = mkdtempSync(join(tmpdir(), "aip-doc-examples-"));
test.after(() => rmSync(dir, { recursive: true, force: true }));

test("문서의 완결된 ```aip 예제는 aip service check를 통과한다", () => {
  let checked = 0;
  const failures = [];
  for (const doc of docs) {
    for (const { line, source } of blocks(readFileSync(join(root, doc), "utf8"))) {
      const first = source.split("\n").find((l) => l.trim() !== "" && !l.trim().startsWith("//")) ?? "";
      if (!TOP_LEVEL.test(first)) continue;
      const needsActor = !/^actor\s+\w+/m.test(source);
      const text = needsActor ? (/^resource Member\b/m.test(source) ? source : PRELUDE + source) : source;
      const file = join(dir, `${checked}.aip`);
      writeFileSync(file, text);
      const run = spawnSync(binary, ["service", "check", file], { encoding: "utf8" });
      checked += 1;
      if (run.status !== 0) {
        const body = JSON.parse(run.stderr || run.stdout);
        failures.push(`${doc}:${line} ${body.diagnostics.map((d) => `${d.code}(${d.message})`).join(", ")}`);
      }
    }
  }
  assert.ok(checked >= 10, `검사한 예제가 너무 적음: ${checked}`);
  assert.deepEqual(failures, []);
});

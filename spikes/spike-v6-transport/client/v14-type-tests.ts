import { connectTypedApply } from "./typed.ts";
import { contract as decimal, type Id as StringId } from "./generated-v14-string.ts";
import { contract as numeric, type Id as NumberId } from "./generated-v14-safe.ts";

const strings = connectTypedApply("http://localhost", "token", decimal);
const numbers = connectTypedApply("http://localhost", "token", numeric);
type Equal<A, B> = (<T>() => T extends A ? 1 : 2) extends (<T>() => T extends B ? 1 : 2) ? true : false;

async function examples() {
  const rows = await strings.read({ read: "Inbox", select: ["id", "checked"] });
  const marked = await strings.apply({ apply: "Inbox.mark", target: { ids: [rows.rows[0].id] } });
  if (marked.ok) {
    const proof: Equal<typeof marked.changed[number], StringId<"Inbox">> = true;
    const tag: string = marked.tags[0];
    const replayed: boolean | undefined = marked.replayed;
    const recovered: boolean | undefined = marked.recovered;
    // @ts-expect-error 결과 Id 배열은 readonly
    marked.changed.push("42");
    // @ts-expect-error 결과 outcome는 readonly
    marked.ok = false;
    void [proof, tag, replayed, recovered];
  } else {
    const code: string = marked.code;
    const recovered: boolean | undefined = marked.recovered;
    // @ts-expect-error 실패에는 changed가 보장되지 않음
    marked.changed;
    void [code, recovered];
  }
  const writeOnly = await strings.apply({ apply: "WriteOnly.mark", target: { ids: ["7"] } });
  if (writeOnly.ok) {
    const proof: Equal<typeof writeOnly.unchanged[number], StringId<"WriteOnly">> = true;
    void proof;
  }
  await strings.apply({ apply: "Inbox.mark", target: { where: [{ field: "checked", op: "eq", value: false }, { field: "count", op: "eq", value: 17 }] } });
  await strings.apply({ apply: "WhereOnly.mark", target: { where: [{ field: "title", op: "eq", value: "one" }] } });
  await strings.apply({ apply: "Inbox.mark", target: { where: [] } });
  const numeric = await numbers.apply({ apply: "Inbox.mark", target: { ids: [42] } });
  if (numeric.ok) {
    const proof: Equal<typeof numeric.changed[number], NumberId<"Inbox">> = true;
    void proof;
  }
  const recovered = await strings.retryPending();
  for (const outcome of recovered) {
    const key: string = outcome.key;
    if (outcome.ok) {
      const proof: Equal<typeof outcome.changed[number], StringId<"Inbox"> | StringId<"WriteOnly"> | StringId<"WhereOnly">> = true;
      void proof;
    }
    void key;
  }
}

// @ts-expect-error 읽기 공개 없는 resource는 apply만 가능
strings.read({ read: "WriteOnly", select: ["id"] });
// @ts-expect-error 닫힌 action
strings.apply({ apply: "Inbox.hidden", target: { ids: ["42"] } });
// @ts-expect-error 없는 action
strings.apply({ apply: "NoSuch.mark", target: { ids: ["42"] } });
// @ts-expect-error target 없이 호출 불가
strings.apply({ apply: "Inbox.mark" });
// @ts-expect-error 비어 있는 target 객체
strings.apply({ apply: "Inbox.mark", target: {} });
// @ts-expect-error id/where 동시 지정 불가
strings.apply({ apply: "Inbox.mark", target: { ids: ["42"], where: [] } });
// @ts-expect-error id 전용 action에 where 불가
strings.apply({ apply: "WriteOnly.mark", target: { where: [] } });
// @ts-expect-error where 전용 action에 ids 불가
strings.apply({ apply: "WhereOnly.mark", target: { ids: ["9"] } });
// @ts-expect-error 닫힌 where 필드
strings.apply({ apply: "Inbox.mark", target: { where: [{ field: "member", op: "eq", value: "1" }] } });
// @ts-expect-error 읽기에 열린 gte는 쓰기에서는 지원하지 않음
strings.apply({ apply: "Inbox.mark", target: { where: [{ field: "count", op: "gte", value: 17 }] } });
// @ts-expect-error Bool 값 타입
strings.apply({ apply: "Inbox.mark", target: { where: [{ field: "checked", op: "eq", value: "false" }] } });
// @ts-expect-error 문자열 모드에 숫자 Id
strings.apply({ apply: "Inbox.mark", target: { ids: [42] } });
// @ts-expect-error 숫자 모드에 문자열 Id
numbers.apply({ apply: "Inbox.mark", target: { ids: ["42"] } });
// @ts-expect-error where Id도 문자열 모드와 일치
strings.apply({ apply: "Inbox.mark", target: { where: [{ field: "id", op: "eq", value: 42 }] } });

// @ts-expect-error outer request의 오타
strings.apply({ apply: "Inbox.mark", target: { ids: ["42"] }, unknownRequest: true });
// @ts-expect-error target의 오타
strings.apply({ apply: "Inbox.mark", target: { ids: ["42"], unknownTarget: true } });
// @ts-expect-error where 항목의 오타
strings.apply({ apply: "Inbox.mark", target: { where: [{ field: "title", op: "eq", value: "one", unknownFilter: true }] } });

void examples;

// 반복 규칙에 따라 다음 마감일을 계산하는 업무 로직. 표준 patch 전이로는 "규칙 계산"을 표현할 수 없다.
const DAY = 86400000;

function nextDue(from, rule) {
  const d = new Date(from.getTime());
  if (rule === "DAILY") return new Date(d.getTime() + DAY);
  if (rule === "WEEKLY") return new Date(d.getTime() + 7 * DAY);
  let n = new Date(d.getTime() + DAY);
  while (n.getUTCDay() === 0 || n.getUTCDay() === 6) n = new Date(n.getTime() + DAY);
  return n;
}

export async function reschedule(input, ctx) {
  const page = await ctx.data.read({
    read: "TodoItem",
    select: ["id", "dueAt", "version"],
    filter: [{ field: "id", op: "eq", value: input.id }],
  });
  const row = page.rows[0];
  if (!row) throw new Error("item not visible");
  const base = row.dueAt ? new Date(row.dueAt) : new Date();
  const due = nextDue(base, input.rule).toISOString().replace(/\.\d{3}Z$/, "Z");
  // PostgreSQL 동기 write 확장에서는 문자열 key를 주면 WIRE.EXTENSION_ERROR로 거부된다(undefined만 통과). 바깥 /apply key가 멱등을 맡는다.
  await ctx.data.apply("TodoItem", "edit", [input.id], undefined, { dueAt: due }, { version: row.version });
  return { id: input.id, dueAt: due };
}

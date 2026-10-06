import { connectTyped } from "./typed.ts";
import { contract } from "./generated-v12.ts";

const client = connectTyped("http://type-test.invalid", "token", contract);

export async function negativeControl() {
  // @ts-expect-error typed clients cannot inject unchecked cache rows
  client.cache.read({}, async () => ({ rows: [], deps: [], maxAgeMs: 1000 }));
  // @ts-expect-error status is not caller-selectable
  await client.read({ read: "Recruitment", select: ["status"] });
  // @ts-expect-error MemberAlarm does not expose this filter
  await client.read({ read: "MemberAlarm", select: ["id"], filter: [{ field: "id", op: "eq", value: "1" }] });
}

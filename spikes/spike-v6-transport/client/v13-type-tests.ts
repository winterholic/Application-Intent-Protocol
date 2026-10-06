import { connectTyped } from "./typed.ts";
import { contract as safeContract, type Id as SafeId } from "./generated-v13-safe.ts";
import { contract as stringContract, type Id as StringId } from "./generated-v13-string.ts";

const strings = connectTyped("http://type.invalid", "token", stringContract);
const numbers = connectTyped("http://type.invalid", "token", safeContract);

export async function typeCases() {
  const text = await strings.read({ read: "Item", select: ["id", "member", "parent", "count"] });
  const id: StringId<"Item"> = text.rows[0].id;
  const member: string = text.rows[0].member;
  const parent: string | null = text.rows[0].parent;
  const count: number = text.rows[0].count;
  await strings.read({ read: "Item", select: ["id"], filter: [{ field: "id", op: "eq", value: id }] });
  // @ts-expect-error decimal-string Id is not a number
  const wrongNumber: number = id;
  // @ts-expect-error decimal-string Id filter cannot receive a number
  await strings.read({ read: "Item", select: ["id"], filter: [{ field: "id", op: "eq", value: 42 }] });
  const numeric = await numbers.read({ read: "Item", select: ["id"] });
  const numericId: SafeId<"Item"> = numeric.rows[0].id;
  const n: number = numericId;
  await numbers.read({ read: "Item", select: ["id"], filter: [{ field: "id", op: "eq", value: numericId }] });
  // @ts-expect-error safe-number Id filter cannot receive a decimal string
  await numbers.read({ read: "Item", select: ["id"], filter: [{ field: "id", op: "eq", value: "42" }] });
  void member; void parent; void count; void n; void wrongNumber;
}

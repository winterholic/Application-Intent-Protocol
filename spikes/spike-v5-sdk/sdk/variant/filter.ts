import { client, type Transport } from "./aip.ts";
declare const send: Transport;
const aip = client(send);
export async function f() {
  await aip.read({ read: "Recruitment", select: ["id"], filter: [{ field: "periodEnd", op: "gte", value: "2026-10-09T00:00:00Z" }] });
  await aip.read({ read: "Recruitment", select: ["id"], filter: [{ field: "id", op: "eq", value: "100" }] });
  await aip.read({ read: "Recruitment", select: ["id"], filter: [{ field: "id", op: "eq", value: 100 }] });
  // @ts-expect-error periodEnd는 select에 없으니 결과 타입에 없다
  const r = await aip.read({ read: "Recruitment", select: ["periodEnd"] });
}

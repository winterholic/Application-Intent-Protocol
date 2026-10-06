import { connectTyped } from "./typed.ts";
import { contract, contractFingerprint } from "./generated-v12.ts";
import type { Id } from "./generated-v12.ts";

type Equal<X, Y> = (<T>() => T extends X ? 1 : 2) extends <T>() => T extends Y ? 1 : 2 ? true : false;
type Expect<T extends true> = T;

const client = connectTyped("http://type-test.invalid", "token", contract);
const fingerprint: string = contractFingerprint;
void fingerprint;

export async function typeCases() {
  const pendingKeys: string[] = client.pending();
  const retried = await client.retryPending();
  void pendingKeys;
  void retried;

  const alarms = await client.read({
    read: "MemberAlarm",
    select: ["id", "isChecked"],
    sort: [{ field: "id", dir: "asc" }],
  });
  type Alarm = (typeof alarms.rows)[number];
  type _alarm = Expect<Equal<Alarm, { readonly id: Id<"MemberAlarm">; readonly isChecked: boolean }>>;
  const alarmId: Id<"MemberAlarm"> = alarms.rows[0].id;
  const checked: boolean = alarms.rows[0].isChecked;
  void alarmId;
  void checked;
  const cacheState: boolean = alarms.cached;
  const staleState: boolean = alarms.stale;
  const storedState: boolean | undefined = alarms.stored;
  void cacheState;
  void staleState;
  void storedState;

  // @ts-expect-error immutable rows mirror the V5 cache's frozen return values
  alarms.rows[0].isChecked = true;

  const recruitment = await client.read({
    read: "Recruitment",
    select: ["id", "title", { club: { select: ["name", "logo"] } }],
  });
  type Recruitment = (typeof recruitment.rows)[number];
  type _recruitment = Expect<Equal<Recruitment, {
    readonly id: Id<"Recruitment">;
    readonly title: string;
    readonly club: { readonly name: string; readonly logo: string | null } | null;
  }>>;
  const title: string = recruitment.rows[0].title;
  const clubName: string | undefined = recruitment.rows[0].club?.name;
  void title;
  void clubName;

  const conditional = Math.random() > 0.5;
  const union = await client.read({ read: "Recruitment", select: ["id", conditional ? "title" : "views"] });
  type Union = (typeof union.rows)[number];
  type _union = Expect<Equal<Union,
    | { readonly id: Id<"Recruitment">; readonly title: string }
    | { readonly id: Id<"Recruitment">; readonly views: number }
  >>;

  const dynamicSelect: ("id" | "title")[] = ["id", "title"];
  const dynamic = await client.read({ read: "Recruitment", select: dynamicSelect });
  const optionalTitle: string | undefined = dynamic.rows[0].title;
  void optionalTitle;

  // @ts-expect-error status exists on the server but is closed to callers
  await client.read({ read: "Recruitment", select: ["status"] });
  // @ts-expect-error relation target's school is not exposed through this edge
  await client.read({ read: "Recruitment", select: [{ club: { select: ["school"] } }] });
  // @ts-expect-error Club is relation-only and cannot be a query root
  await client.read({ read: "Club", select: ["id"] });
  // @ts-expect-error periodEnd.gte requires a string Time value
  await client.read({ read: "Recruitment", select: ["id"], filter: [{ field: "periodEnd", op: "gte", value: 5 }] });
  // @ts-expect-error title is not in Recruitment's filter allowlist
  await client.read({ read: "Recruitment", select: ["id"], filter: [{ field: "title", op: "eq", value: "x" }] });
}
